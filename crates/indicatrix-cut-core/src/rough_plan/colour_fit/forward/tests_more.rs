//! More tests of the forward tracer (written, not run, by lane F1): the disk cache, record
//! compression, the white furnace, the Jacobian, the light model and small utilities.

use std::sync::atomic::AtomicBool;

use glam::DVec3;

use super::{
    CacheStatus, ForwardInput, ForwardOptions, ForwardRecords, ForwardStats, PathRecord, PixelLoss,
    RigLighting, SpectralModel, SpectralSetup, StoneIndex, SurfaceClass, SurfaceMap, ViewRecords,
    ViewTraceInput, WhiteFrame, build_spectral, cache,
    compress::compress,
    evaluate, evaluate_with_jacobian, for_each_pixel_jacobian,
    parallel::{run_indexed, thread_count},
    rng::{PixelSequence, Rng},
    tests::{Fixture, bicolour, centre_grid, colour_rig, cube_fixture, mean_channel, ortho_pose},
    trace_rig,
};
use crate::rough_plan::{
    camera_spectral::{CameraResponse, GRID_LEN, ResponseTier},
    locate::{RigProfile, box_mesh},
    photometry::{LinearImage, PixelMask, WorkingGrid, flag},
};

// ---------------------------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------------------------

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("zfwd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn cache_round_trip_and_corrupt_files_are_ignored() {
    let dir = temp_dir("cache");
    let mut fixture = cube_fixture(DVec3::new(-60.0, 30.0, 20.0), 1.5, 1.0);
    fixture.surfaces = SurfaceMap::frosted(0.1);
    fixture.options.samples = 16;
    let mut input: ForwardInput<'_> = fixture.input();
    input.cache_dir = Some(&dir);
    let never = AtomicBool::new(false);

    let first = trace_rig(&input, &never, &mut |_| {}).expect("trace");
    assert_eq!(first.stats.cache, CacheStatus::Stored);
    let key = cache::cache_key(&input);
    let path = cache::cache_path(&dir, key);
    assert!(path.exists());

    let second = trace_rig(&input, &never, &mut |_| {}).expect("cache hit");
    assert_eq!(second.stats.cache, CacheStatus::Hit);
    assert_eq!(first.views, second.views);
    assert_eq!(first.spectral, second.spectral);
    assert_eq!(first.stats.samples, second.stats.samples);

    // A flipped byte fails the checksum: the file is ignored and rewritten.
    let good = std::fs::read(&path).expect("read");
    let mut corrupt = good.clone();
    let middle = corrupt.len() / 2;
    corrupt[middle] ^= 0x5A;
    std::fs::write(&path, &corrupt).expect("write");
    assert!(cache::load(&dir, key).is_none());
    let third = trace_rig(&input, &never, &mut |_| {}).expect("retrace");
    assert_eq!(third.stats.cache, CacheStatus::Stored);
    assert_eq!(first.views, third.views);

    // Truncated, empty, wrong-key and wrong-version files are refused.
    assert!(cache::decode(&good[..good.len() - 9], key).is_none());
    assert!(cache::decode(&[], key).is_none());
    assert!(cache::decode(&good, key ^ 1).is_none());
    let mut old = good.clone();
    old[4] = 99;
    let body = old.len() - 8;
    let mut sum = cache::Fnv::new();
    sum.bytes(&old[..body]);
    old[body..].copy_from_slice(&sum.finish().to_le_bytes());
    assert!(
        cache::decode(&old, key).is_none(),
        "a file of another version is ignored even with a valid checksum"
    );
    assert!(cache::decode(&good, key).is_some());

    // The key follows the inputs.
    let mut other = cube_fixture(DVec3::new(-60.0, 30.0, 20.0), 1.5, 1.0);
    other.surfaces = SurfaceMap::frosted(0.2);
    other.options.samples = 16;
    assert_ne!(
        cache::cache_key(&other.input()),
        key,
        "roughness is in the key"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------------------------

struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1_u64 << 53) as f64
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the record storage precision; the generated lengths, weights and chroma are small"
)]
fn random_records(count: usize, spread0: f64, spread1: f64) -> Vec<PathRecord> {
    let mut rng = Lcg(7);
    (0..count)
        .map(|_| PathRecord {
            lengths: [
                spread0.mul_add(rng.unit(), 4.0) as f32,
                spread1.mul_add(rng.unit(), 1.0) as f32,
                0.0,
                0.0,
                0.0,
            ],
            weight: (0.2 + rng.unit()) as f32 / count as f32,
            // A colour that follows the path (a panel gradient): red up, blue down, mean 1.
            chroma: {
                let tilt = (rng.unit() - 0.5) * 0.4;
                [(1.0 + tilt) as f32, 1.0, (1.0 - tilt) as f32]
            },
        })
        .collect()
}

fn transmitted(records: &[PathRecord], alpha: [f64; 2]) -> f64 {
    records
        .iter()
        .map(|r| {
            let tau = alpha[1].mul_add(f64::from(r.lengths[1]), alpha[0] * f64::from(r.lengths[0]));
            f64::from(r.weight) * (-tau).exp()
        })
        .sum()
}

#[test]
fn compression_preserves_the_moments_and_bounds_the_transmittance_error() {
    let raw = random_records(600, 2.0, 0.2);
    let alpha_ref = 0.3_f64;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the compression tolerance precision; 0.1 / 0.3 is exact enough"
    )]
    let tolerance = (0.1 / alpha_ref) as f32;
    let mut work = raw.clone();
    let mut out = Vec::new();
    let extent = compress(&mut work, 16, tolerance, 2, &mut out);
    assert!(out.len() <= 16 && out.len() > 1, "{} clusters", out.len());
    assert!(
        extent <= tolerance,
        "extent {extent} within the tolerance {tolerance}"
    );

    let weight = |records: &[PathRecord]| records.iter().map(|r| f64::from(r.weight)).sum::<f64>();
    let moment = |records: &[PathRecord], z: usize| {
        records
            .iter()
            .map(|r| f64::from(r.weight) * f64::from(r.lengths[z]))
            .sum::<f64>()
    };
    assert!((weight(&out) / weight(&raw) - 1.0).abs() < 1e-5);
    // The weight of every colour channel is preserved as well (ZC-13).
    let channel = |records: &[PathRecord], c: usize| {
        records
            .iter()
            .map(|r| f64::from(r.weight) * f64::from(r.chroma[c]))
            .sum::<f64>()
    };
    for c in 0..3 {
        assert!(
            (channel(&out, c) / channel(&raw, c) - 1.0).abs() < 1e-5,
            "channel {c}"
        );
    }
    for z in 0..2 {
        assert!(
            (moment(&out, z) / moment(&raw, z) - 1.0).abs() < 1e-5,
            "zone {z}"
        );
    }
    for alpha in [[0.3, 0.1], [0.05, 0.3], [0.3, 0.0]] {
        let exact = transmitted(&raw, alpha);
        let approx = transmitted(&out, alpha);
        assert!(
            (approx / exact - 1.0).abs() < 5e-3,
            "alpha {alpha:?}: {approx} against {exact}"
        );
    }

    // The input order does not matter.
    let mut reversed: Vec<PathRecord> = raw.iter().rev().copied().collect();
    let mut again = Vec::new();
    compress(&mut reversed, 16, tolerance, 2, &mut again);
    assert_eq!(out, again);
}

#[test]
fn identical_paths_compress_to_one_record() {
    let record = PathRecord {
        lengths: [3.5, 0.0, 0.0, 0.0, 0.0],
        weight: 0.25,
        chroma: [1.0; 3],
    };
    let mut raw = vec![record; 40];
    let mut out = Vec::new();
    assert_eq!(compress(&mut raw, 16, 0.3, 1, &mut out), 0.0);
    assert_eq!(out.len(), 1);
    assert!((out[0].weight - 10.0).abs() < 1e-5);
    assert!((out[0].lengths[0] - 3.5).abs() < 1e-6);
    assert_eq!(out[0].chroma, [1.0; 3], "grey records stay exactly grey");
}

// ---------------------------------------------------------------------------------------------
// White furnace
// ---------------------------------------------------------------------------------------------

#[test]
fn a_frosted_plate_in_a_white_furnace_conserves_energy() {
    // A thin plate seen at about 37 degrees from its large face, lit from everywhere with
    // level 1 and not absorbing: everything that enters comes out again, either reflected
    // towards the camera or transmitted.
    let direction = DVec3::new(0.8, 0.5, 0.33).normalize();
    let pose = ortho_pose(-100.0 * direction);
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.mesh = box_mesh(DVec3::new(0.5, 10.0, 10.0)).expect("a plate");
    fixture.rig = colour_rig(&pose, 1.5, 1.0, RigLighting::uniform_surround());
    fixture.surfaces = SurfaceMap::frosted(0.05);
    fixture.options.samples = 256;
    let records = fixture.trace();
    for c in 0..3 {
        let mean = mean_channel(&records, 0.0, c);
        assert!(
            (mean - 1.0).abs() < 0.03,
            "channel {c}: the furnace gives {mean}"
        );
    }
    assert!(records.lost_depth_fraction() < 0.01);
    assert!(records.valid_pixels() >= 32);
}

// ---------------------------------------------------------------------------------------------
// Jacobian
// ---------------------------------------------------------------------------------------------

/// `alpha_z(lambda) = p[2z] + p[2z + 1] u`, `u = (lambda - 550) / 150`.
struct TiltModel;

impl SpectralModel for TiltModel {
    fn n_params(&self) -> usize {
        4
    }

    fn alpha(&self, zone: usize, lambda_nm: f64, params: &[f64]) -> f64 {
        let u = (lambda_nm - 550.0) / 150.0;
        params[2 * zone + 1].mul_add(u, params[2 * zone])
    }

    fn dalpha(&self, zone: usize, lambda_nm: f64, _params: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        out[2 * zone] = 1.0;
        out[2 * zone + 1] = (lambda_nm - 550.0) / 150.0;
    }
}

#[test]
fn the_jacobian_matches_finite_differences() {
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.zones = Some(bicolour());
    fixture.options.samples = 32;
    fixture.index = StoneIndex::Dispersion(
        indicatrix::optics::dispersion::DispersionModel::Sellmeier1 { b1: 1.2, c1: 0.01 },
    );
    fixture.options.trace_dispersion = true;
    let records = fixture.trace();
    let params = [0.10, 0.04, 0.35, -0.05];

    let mut rows: Vec<([f64; 3], Vec<f64>)> = Vec::new();
    for_each_pixel_jacobian(&records, &params, &TiltModel, &mut |pixel| {
        if rows.len() < 6 {
            rows.push((pixel.rgb, pixel.jacobian.to_vec()));
        }
    })
    .expect("evaluates");
    assert_eq!(rows.len(), 6);

    let h = 1e-5;
    for j in 0..4 {
        let at = |delta: f64| {
            let mut p = params;
            p[j] += delta;
            let mut out: Vec<[f64; 3]> = Vec::new();
            for_each_pixel_jacobian(&records, &p, &TiltModel, &mut |pixel| {
                if out.len() < 6 {
                    out.push(pixel.rgb);
                }
            })
            .expect("evaluates");
            out
        };
        let (plus, minus) = (at(h), at(-h));
        for (k, (_, jac)) in rows.iter().enumerate() {
            for c in 0..3 {
                let numeric = (plus[k][c] - minus[k][c]) / (2.0 * h);
                let analytic = jac[c * 4 + j];
                assert!(
                    (numeric - analytic).abs() < 1e-4f64.mul_add(analytic.abs(), 1e-6),
                    "pixel {k} channel {c} parameter {j}: {analytic} against {numeric}"
                );
            }
        }
    }

    // The materialised form agrees with the visitor, and with `evaluate`.
    let full = evaluate_with_jacobian(&records, &params, &TiltModel).expect("evaluates");
    assert_eq!(full.jacobians.len(), 1);
    assert_eq!(full.jacobians[0].n_params, 4);
    let plain = evaluate(&records, &|z, l| TiltModel.alpha(z, l, &params));
    assert_eq!(plain[0].rgb, full.predictions[0].rgb);
    let first_valid = full.predictions[0]
        .valid
        .iter()
        .position(|&v| v)
        .expect("a valid pixel");
    let (rgb, jac) = &rows[0];
    for c in 0..3 {
        assert!((f64::from(full.predictions[0].rgb[first_valid][c]) - rgb[c]).abs() < 1e-5);
        for j in 0..4 {
            let stored = f64::from(full.jacobians[0].data[(first_valid * 3 + c) * 4 + j]);
            assert!((stored - jac[c * 4 + j]).abs() < 1e-4f64.mul_add(jac[c * 4 + j].abs(), 1e-5));
        }
    }
    assert!(evaluate_with_jacobian(&records, &params[..3], &TiltModel).is_err());
}

// ---------------------------------------------------------------------------------------------
// Light model
// ---------------------------------------------------------------------------------------------

#[test]
fn the_white_frame_is_bilinear_and_marks_unseen_exit_points() {
    // A 4 x 4 frame whose level rises along x: level = 1 + column.
    let pixels: Vec<[f32; 3]> = (0..16)
        .map(|i| {
            let v = 1.0 + (i % 4) as f32;
            [v, v, v]
        })
        .collect();
    let image =
        LinearImage::from_pixels(4, 4, pixels, crate::rough_plan::photometry::SourceKind::Raw)
            .expect("an image");
    let frame = WhiteFrame::from_corrected(&image, [1.0; 3], 4).expect("a frame");
    assert_eq!(frame.cells(), (4, 4, 1.0));
    // At a pixel centre the level is the pixel's own; halfway between centres, the mean.
    for c in 0..3 {
        assert!((frame.level_at(1.5, 2.5).unwrap()[c] - 2.0).abs() < 1e-6);
        assert!((frame.level_at(2.0, 2.5).unwrap()[c] - 2.5).abs() < 1e-6);
    }
    assert!(frame.level_at(-0.1, 1.0).is_none());
    assert!(frame.level_at(4.0, 1.0).is_none());

    // A rig whose image is the 200 x 200 default: a uniform panel level 2.
    let pose = ortho_pose(DVec3::new(-100.0, 0.0, 0.0));
    let uniform = WhiteFrame::from_levels([200, 200], 1.0, 200, 200, vec![[2.0; 3]; 200 * 200])
        .expect("a frame");
    let lighting = RigLighting::backlight(
        vec![super::PanelGeom::facing_camera(
            &pose,
            200.0,
            [400.0, 400.0],
        )],
        vec![Some(uniform)],
    );
    let rig = RigProfile::new("rig", vec![pose], 1.5);
    let own = [2.0; 3];
    // Through the middle: the camera saw that panel point, so the ratio is 1.
    let seen = lighting.exit_light(&rig, 0, DVec3::ZERO, DVec3::X, own);
    assert!(seen.ratio.iter().all(|r| (r - 1.0).abs() < 1e-9) && !seen.flagged);
    // Sideways by 40 mm: that panel point projects outside the 20 mm wide image: flagged mean.
    let unseen = lighting.exit_light(&rig, 0, DVec3::new(0.0, 40.0, 0.0), DVec3::X, own);
    assert!(unseen.ratio.iter().all(|r| (r - 1.0).abs() < 1e-9) && unseen.flagged);
    // Away from the panel and past its edge: dark.
    assert_eq!(
        lighting
            .exit_light(&rig, 0, DVec3::ZERO, -DVec3::X, own)
            .ratio,
        [0.0; 3]
    );
    assert_eq!(
        lighting
            .exit_light(&rig, 0, DVec3::new(0.0, 500.0, 0.0), DVec3::X, own)
            .ratio,
        [0.0; 3]
    );
    // A dead channel at the camera ray's own pixel leaves no defined transmittance: dark.
    assert_eq!(
        lighting
            .exit_light(&rig, 0, DVec3::ZERO, DVec3::X, [2.0, 0.0, 2.0])
            .ratio,
        [0.0; 3]
    );
}

/// A 200 x 200 white frame (one cell per photo pixel) whose three channels are `level(c, x, y)`.
fn graded_frame(level: impl Fn(usize, f64, f64) -> f32) -> WhiteFrame {
    let cells: Vec<[f32; 3]> = (0..200 * 200)
        .map(|i| {
            let (x, y) = (f64::from(i % 200) + 0.5, f64::from(i / 200) + 0.5);
            [level(0, x, y), level(1, x, y), level(2, x, y)]
        })
        .collect();
    WhiteFrame::from_levels([200, 200], 1.0, 200, 200, cells).expect("a frame")
}

/// The exit ratio is taken per channel: with a panel that is redder on the right and bluer on the
/// left, the ratio of each channel is that channel's own level at the exit point over its level at
/// the camera ray's pixel (ZC-13). The photo point `(u, v)` of the panel is the pixel the camera
/// ray started from, and the exit ray through it reaches the panel at pixel `(u + 20, v)`.
#[test]
fn the_exit_ratio_follows_each_channel_of_the_white_frame() {
    let pose = ortho_pose(DVec3::new(-100.0, 0.0, 0.0));
    // The red level rises 0.4 across the photo, the blue falls by 0.4, the green stays 1.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the white-frame level precision; the levels are within 0.8..=1.2"
    )]
    let ramp = |c: usize, x: f64, _y: f64| match c {
        0 => (0.8 + 0.4 * x / 200.0) as f32,
        2 => (1.2 - 0.4 * x / 200.0) as f32,
        _ => 1.0,
    };
    let frame = graded_frame(ramp);
    let lighting = RigLighting::backlight(
        vec![super::PanelGeom::facing_camera(
            &pose,
            200.0,
            [400.0, 400.0],
        )],
        vec![Some(frame)],
    );
    let rig = RigProfile::new("rig", vec![pose.clone()], 1.5);
    // An exit ray from the origin parallel to the view axis hits the panel at the pixel the camera
    // sees the origin at; one displaced by 1 mm sideways hits it 10 pixels away (10 px/mm).
    let (own_pixel, other_pixel) = {
        let centre = pose.project(DVec3::ZERO).expect("the origin is in view");
        let shifted = pose
            .project(DVec3::new(0.0, 1.0, 0.0))
            .expect("the shifted point is in view");
        (centre, shifted)
    };
    let own = lighting.own_level(0, own_pixel);
    let light = lighting.exit_light(&rig, 0, DVec3::new(0.0, 1.0, 0.0), DVec3::X, own);
    assert!(!light.flagged);
    for (c, &ratio) in light.ratio.iter().enumerate() {
        let at = |x: f64| f64::from(ramp(c, x, 0.0));
        let expected = at(other_pixel.x) / at(own_pixel.x);
        assert!(
            (ratio - expected).abs() < 1e-5,
            "channel {c}: {ratio} against {expected}"
        );
    }
    let [red, green, blue] = light.ratio;
    assert!(
        (red - 1.0) * (blue - 1.0) < 0.0 && (green - 1.0).abs() < 1e-9,
        "red and blue move in opposite directions, green stays: {:?}",
        light.ratio
    );
}

#[test]
fn a_mostly_unseen_pixel_is_dropped_for_its_lambertian_light() {
    // The panel level image covers only a small part of the photo: every exit point that the
    // camera never saw gets the mean level and is flagged, so the pixel is dropped.
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.5);
    let pose = ortho_pose(DVec3::new(-100.0, 0.0, 0.0));
    // A photo of 50 x 50 pixels only, while the camera model is 200 x 200: the central pixels at
    // (100, 100) are outside this frame, so their own level is the mean and the exit is flagged.
    let tiny =
        WhiteFrame::from_levels([50, 50], 1.0, 50, 50, vec![[1.0; 3]; 50 * 50]).expect("a frame");
    fixture.rig.lighting = RigLighting::backlight(
        vec![super::PanelGeom::facing_camera(
            &pose,
            200.0,
            [400.0, 400.0],
        )],
        vec![Some(tiny)],
    );
    let records = fixture.trace();
    let view = &records.views[0];
    assert!((0..view.pixel_count()).all(|p| view.status[p] & super::status::FLAGGED_LIGHT != 0));
    assert_eq!(records.valid_pixels(), 0);
}

// ---------------------------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------------------------

#[test]
fn surface_map_overrides_stay_sorted_and_replace() {
    let map = SurfaceMap::frosted(0.2)
        .with_override(9, SurfaceClass::Polished)
        .with_override(3, SurfaceClass::Frosted { roughness: 0.4 })
        .with_override(9, SurfaceClass::Frosted { roughness: 0.1 });
    assert_eq!(map.overrides().len(), 2);
    assert!(map.overrides().windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(map.class_of(9), SurfaceClass::Frosted { roughness: 0.1 });
    assert_eq!(map.class_of(3), SurfaceClass::Frosted { roughness: 0.4 });
    assert_eq!(map.class_of(5), SurfaceClass::Frosted { roughness: 0.2 });
}

#[test]
fn counter_based_numbers_depend_only_on_their_key() {
    let mut first_rng = Rng::keyed(1, 2, 3, 4, 5);
    let mut same_key = Rng::keyed(1, 2, 3, 4, 5);
    let mut other_key = Rng::keyed(1, 2, 3, 5, 5);
    let first: Vec<f64> = (0..4).map(|_| first_rng.next_f64()).collect();
    assert_eq!(
        first,
        (0..4).map(|_| same_key.next_f64()).collect::<Vec<_>>()
    );
    assert_ne!(first[0], other_key.next_f64());
    assert!(first.iter().all(|v| (0.0..1.0).contains(v)));

    // The R3 points of a pixel stay in the unit cube and spread out.
    let sequence = PixelSequence::new(0, 1, 2);
    let mut cells = [false; 16];
    for index in 0..64 {
        let [px, py, lambda] = sequence.point(index);
        assert!(
            (0.0..1.0).contains(&px) && (0.0..1.0).contains(&py) && (0.0..1.0).contains(&lambda)
        );
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "px and py are asserted to be in 0..1 just above, so the cell indices are in 0..4"
        )]
        let cell = (px * 4.0) as usize * 4 + (py * 4.0) as usize;
        cells[cell] = true;
    }
    assert!(cells.iter().all(|&hit| hit), "64 points reach all 16 cells");
}

#[test]
fn indexed_runs_return_results_in_order_for_any_thread_count() {
    let cancel = AtomicBool::new(false);
    for threads in [1, 3, 16] {
        let results = run_indexed(50, threads, &cancel, &|i| i * i, &mut |_| {});
        assert!(results.iter().enumerate().all(|(i, r)| *r == Some(i * i)));
    }
    assert_eq!(thread_count(16, 3), 3);
    assert_eq!(thread_count(0, 0), 1);
    let stop = AtomicBool::new(true);
    let none = run_indexed(5, 2, &stop, &|i| i, &mut |_| {});
    assert!(none.iter().all(Option::is_none));
}

#[test]
fn cancelling_returns_an_error_and_caches_nothing() {
    let dir = temp_dir("cancel");
    let fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    let mut input = fixture.input();
    input.cache_dir = Some(&dir);
    let stop = AtomicBool::new(true);
    let result = trace_rig(&input, &stop, &mut |_| {});
    assert_eq!(result.err(), Some(super::ForwardError::Cancelled));
    assert!(!cache::cache_path(&dir, cache::cache_key(&input)).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_input_is_refused() {
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.views = Vec::new();
    assert_eq!(
        trace_rig(&fixture.input(), &AtomicBool::new(false), &mut |_| {}).err(),
        Some(super::ForwardError::NoViews)
    );
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.views = vec![super::ViewTraceInput::new(3, centre_grid())];
    assert!(matches!(
        trace_rig(&fixture.input(), &AtomicBool::new(false), &mut |_| {}).err(),
        Some(super::ForwardError::BadView(0))
    ));
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.options.samples = 1;
    assert!(matches!(
        trace_rig(&fixture.input(), &AtomicBool::new(false), &mut |_| {}).err(),
        Some(super::ForwardError::BadOptions(_))
    ));
    // A camera that sees nothing in the range.
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    let mut sensitivity = [[0.0; GRID_LEN]; 3];
    sensitivity[0][0] = 1.0;
    fixture.camera =
        CameraResponse::from_grid(sensitivity, ResponseTier::Measured).expect("finite");
    assert!(matches!(
        trace_rig(&fixture.input(), &AtomicBool::new(false), &mut |_| {}).err(),
        Some(super::ForwardError::BadSpectrum(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// Spectral quadrature (ZC-2)
// ---------------------------------------------------------------------------------------------

/// Records of a 64-pixel view whose pixel 0 holds one base-zone record of `length_mm` and weight
/// 1; every other pixel is skipped.
fn single_record_records(spectral: SpectralSetup, length_mm: f32) -> ForwardRecords {
    let grid = centre_grid();
    let n = grid.width * grid.height;
    let mut status = vec![super::status::SKIPPED; n];
    status[0] = 0;
    let mut offsets = vec![0_u32];
    offsets.extend(std::iter::repeat_n(1_u32, n));
    ForwardRecords {
        spectral,
        n_zones: 1,
        views: vec![ViewRecords {
            view: 0,
            grid,
            status,
            samples: vec![1; n],
            mc_mean: vec![1.0; n],
            mc_variance: vec![0.0; n],
            mc_mean_clear: vec![1.0; n],
            mc_variance_clear: vec![0.0; n],
            loss: vec![PixelLoss::default(); n],
            offsets,
            stride: 2,
            records: vec![length_mm, 1.0],
            chroma: Vec::new(),
        }],
        stats: ForwardStats {
            cache: CacheStatus::NotUsed,
            samples: 1,
            max_cluster_range_mm: 0.0,
        },
    }
}

/// The transmittance of a Gaussian band at `lambda_nm`: optical depth `tau` at its peak.
fn gaussian_dip(lambda_nm: f64, centre_nm: f64, sigma_nm: f64, tau: f64) -> f64 {
    let z = (lambda_nm - centre_nm) / sigma_nm;
    (-tau * (-0.5 * z * z).exp()).exp()
}

/// The midpoint rule's error for a narrow band falls off faster than any power of the point
/// spacing `h`: for a Gaussian band inside the range it is the aliasing term
/// `2 exp(-2 pi^2 sigma^2 / h^2)` of the dip, for a band cut by the range edge the end term
/// `(h^2 / 24)` times the slope of the transmittance there (ZC-21; the module notes have the
/// numbers). The default grid (8 points per bin, 4.7 nm) integrates a sigma = 10 nm band of
/// optical depth 1 to better than 0.2 percent of the camera value, an interior band and one cut by
/// the range edge, and a coarser grid is worse.
///
/// The bound asserted per grid, `(h^2 / 24) tau / sigma^2`, is deliberately loose: it is the
/// curvature scale of the band, not the error of the rule, and holds with a wide margin at every
/// spacing (the limits that matter are the 0.2 percent at 8 and 32 points per bin).
#[test]
fn the_default_spectral_quadrature_resolves_a_ten_nanometre_band() {
    assert_eq!(ForwardOptions::default().spectral_sub, 8);
    let (length, sigma, tau) = (10.0_f32, 10.0_f64, 1.0_f64);
    for centre in [550.0_f64, 405.0] {
        // The 0.1 nm midpoint reference of the mean over 400 to 700 nm (flat camera and light).
        let steps = 3000_usize;
        let reference = (0..steps)
            .map(|k| gaussian_dip(0.1_f64.mul_add(k as f64 + 0.5, 400.0), centre, sigma, tau))
            .sum::<f64>()
            / steps as f64;
        let mut errors = Vec::new();
        for sub in [1_usize, 2, 8, 32] {
            let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
            fixture.options.bins = 8;
            fixture.options.spectral_sub = sub;
            let spectral = build_spectral(&fixture.input()).expect("flat spectra");
            let records = single_record_records(spectral, length);
            let predicted = evaluate(&records, &|_, lambda| {
                let z = (lambda - centre) / sigma;
                tau / f64::from(length) * (-0.5 * z * z).exp()
            });
            let value = f64::from(predicted[0].rgb[0][0]);
            let relative = (value / reference - 1.0).abs();
            // The loose curvature-scale bound, relative to the band's own transmittance.
            let h = 300.0 / (8 * sub) as f64;
            assert!(
                relative <= h * h / 24.0 * tau / (sigma * sigma) + 1e-6,
                "centre {centre}, {sub} points per bin: error {relative}"
            );
            errors.push(relative);
        }
        assert!(
            errors[2] < 0.002,
            "centre {centre}: sub 8 error {} (sub 1, 2, 8, 32: {errors:?})",
            errors[2]
        );
        assert!(
            errors[3] < 0.002,
            "centre {centre}: sub 32 error {}",
            errors[3]
        );
        assert!(
            errors[0] > errors[2],
            "centre {centre}: the coarse grid is worse ({} against {})",
            errors[0],
            errors[2]
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The record store (ZC-8, ZC-9)
// ---------------------------------------------------------------------------------------------

/// Three views of a 10 x 10 grid with scattered masked pixels, frosted and zoned: whatever the
/// chunking, the store is a valid CSR cover, no (pixel, bin) exceeds the record budget and the
/// result is the same.
#[test]
fn the_record_store_keeps_its_invariants_for_any_chunking() {
    let mut fixture = cube_fixture(DVec3::new(-60.0, 30.0, 20.0), 1.6, 1.0);
    fixture.surfaces = SurfaceMap::frosted(0.15);
    fixture.zones = Some(bicolour());
    fixture.options.samples = 16;
    fixture.options.max_sample_factor = 2;
    fixture.options.max_records = 6;
    let grid = WorkingGrid::fit([95, 95, 105, 105], 10).expect("a valid region");
    let views: Vec<ViewTraceInput<'static>> = (0..3_usize)
        .map(|k| {
            let mut mask = PixelMask::new(grid.width, grid.height);
            for p in (k..grid.width * grid.height).step_by(7) {
                mask.set(p % grid.width, p / grid.width, flag::USER);
            }
            let mask: &'static PixelMask = Box::leak(Box::new(mask));
            ViewTraceInput::new(0, grid).with_mask(mask)
        })
        .collect();
    fixture.views = views;

    let mut baseline: Option<ForwardRecords> = None;
    for chunk in [1_usize, 3, 7] {
        fixture.options.chunk_pixels = chunk;
        let records = fixture.trace();
        let traced = records.spectral.traced_bins;
        for (slot, view) in records.views.iter().enumerate() {
            view.check_offsets(traced, fixture.options.max_records)
                .unwrap_or_else(|e| panic!("chunk {chunk}, view {slot}: {e}"));
            assert_eq!(view.stride, records.n_zones + 1);
            assert_eq!(view.offsets.len(), view.pixel_count() * traced + 1);
            assert_eq!(view.offsets[0], 0);
            assert_eq!(
                view.offsets.last().copied().map(|o| o as usize),
                Some(view.record_count())
            );
            for p in 0..view.pixel_count() {
                for bin in 0..traced {
                    assert!(
                        view.pixel_records(p, bin, traced).len() <= fixture.options.max_records,
                        "chunk {chunk}, view {slot}, pixel {p}"
                    );
                }
            }
            assert!(view.record_count() > 0, "the frosted cube leaves records");
        }
        match &baseline {
            None => baseline = Some(records),
            Some(first) => assert_eq!(first, &records, "chunks of {chunk} differ"),
        }
    }

    // A broken store is reported, not sliced.
    let mut broken = baseline.expect("traced").views.remove(0);
    let last = broken.offsets.len() - 1;
    broken.offsets[last] += 1;
    assert!(broken.check_offsets(1, 6).is_err(), "the cover is short");
    broken.offsets[last] -= 1;
    broken.offsets[1] = broken.offsets[last] + 1;
    assert!(broken.check_offsets(1, 6).is_err(), "the offsets decrease");
    broken.offsets.pop();
    assert!(broken.check_offsets(1, 6).is_err(), "an offset is missing");
}

/// The compact store costs `n_zones + 1` floats per record: a uniform stone 8 bytes, a bicolour
/// stone 12.
#[test]
fn records_are_stored_with_one_length_per_zone() {
    let uniform = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0).trace();
    let view = &uniform.views[0];
    assert_eq!((uniform.n_zones, view.stride), (1, 2));
    assert_eq!(view.records.len(), view.record_count() * 2);
    assert!(
        view.chroma.is_empty(),
        "a grey panel stores no colour codes"
    );

    let mut zoned = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    zoned.zones = Some(bicolour());
    let zoned = zoned.trace();
    let view = &zoned.views[0];
    assert_eq!((zoned.n_zones, view.stride), (2, 3));
    assert_eq!(view.records.len(), view.record_count() * 3);
    assert!(zoned.memory_bytes() >= view.records.len() * 4);
}

/// A dispersive stone is traced in one shared bin at the d-line index unless asked otherwise.
#[test]
fn dispersion_is_traced_in_one_bin_by_default() {
    let mut fixture = cube_fixture(DVec3::new(-60.0, 30.0, 20.0), 1.5, 1.0);
    fixture.index = StoneIndex::Dispersion(
        indicatrix::optics::dispersion::DispersionModel::Sellmeier1 { b1: 1.2, c1: 0.01 },
    );
    fixture.options.samples = 16;
    let shared = fixture.trace();
    assert_eq!(shared.spectral.traced_bins, 1);
    assert!(shared.spectral.eval_points.iter().all(|p| p.bin == 0));
    assert_eq!(shared.spectral.eval_points.len(), 4 * 8);

    fixture.options.trace_dispersion = true;
    let per_bin = fixture.trace();
    assert_eq!(per_bin.spectral.traced_bins, 4);
    let key_per_bin = cache::cache_key(&fixture.input());
    fixture.options.trace_dispersion = false;
    assert_ne!(
        key_per_bin,
        cache::cache_key(&fixture.input()),
        "the option is part of the cache key"
    );
}

// ---------------------------------------------------------------------------------------------
// Monte-Carlo statistics (ZC-7)
// ---------------------------------------------------------------------------------------------

/// A polished slab: the clear-stone mean is `(1 - R) / (1 + R)`, the mean at the pixel's reference
/// absorption (optical depth 1 at its 10 mm path) is `(1 - R)^2 e^-1` to the multiple-reflection
/// correction, and the variance of a deterministic slab is zero at both.
#[test]
fn the_monte_carlo_statistics_are_kept_at_zero_and_at_unit_optical_depth() {
    let fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    let records = fixture.trace();
    let view = &records.views[0];
    let reflectance = 0.04_f64;
    let clear = (1.0 - reflectance) / (1.0 + reflectance);
    let at_unit_depth = (1.0 - reflectance).powi(2) * (-1.0_f64).exp();
    // A pixel is a few hundred Fresnel-roulette samples, so judge the mean over the pixels.
    let (mut seen, mut sum_clear, mut sum_ref) = (0_usize, 0.0_f64, 0.0_f64);
    for p in 0..view.pixel_count() {
        if !view.is_valid(p) {
            continue;
        }
        let (mean_clear, mean_ref) = (f64::from(view.mc_mean_clear[p]), f64::from(view.mc_mean[p]));
        assert!(
            mean_ref <= mean_clear,
            "pixel {p}: {mean_ref} against {mean_clear}"
        );
        assert!(view.mc_variance_clear[p].is_finite() && view.mc_variance_clear[p] >= 0.0);
        assert!(view.mc_variance[p].is_finite() && view.mc_variance[p] >= 0.0);
        sum_clear += mean_clear;
        sum_ref += mean_ref;
        seen += 1;
    }
    assert!(seen >= 32);
    let (mean_clear, mean_ref) = (sum_clear / seen as f64, sum_ref / seen as f64);
    assert!(
        (mean_clear / clear - 1.0).abs() < 0.01,
        "clear {mean_clear} against {clear}"
    );
    assert!(
        (mean_ref / at_unit_depth - 1.0).abs() < 0.03,
        "reference {mean_ref} against {at_unit_depth}"
    );
}

/// On a long path the clear-stone transmittance is judged, not the one at a fixed absorption that
/// underflows (a 0.3/mm estimate over a 60 mm block is about e^-18, below any absolute error
/// target at once): every pixel that reports itself converged reaches its relative target on the
/// clear transmittance, or has a standard error below the absolute one.
#[test]
fn convergence_is_judged_on_the_clear_stone_transmittance() {
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.mesh = box_mesh(DVec3::splat(30.0)).expect("a block");
    fixture.surfaces = SurfaceMap::frosted(0.2);
    fixture.options.samples = 32;
    fixture.options.max_sample_factor = 4;
    let records = fixture.trace();
    let view = &records.views[0];
    let target = fixture.options.target_rel_error;
    let mut valid = 0;
    for p in 0..view.pixel_count() {
        if !view.is_valid(p) {
            continue;
        }
        valid += 1;
        let relative = if view.mc_mean_clear[p] > 0.0 {
            f64::from(view.mc_variance_clear[p]).sqrt() / f64::from(view.mc_mean_clear[p])
        } else {
            0.0
        };
        let converged = view.status[p] & super::status::NOT_CONVERGED == 0;
        assert!(
            !converged
                || relative <= target * 1.0001
                || f64::from(view.mc_variance_clear[p]).sqrt() <= 1.0001e-4,
            "pixel {p}: converged with a relative error {relative}"
        );
    }
    assert!(valid > 0, "the block leaves usable pixels");
}

// ---------------------------------------------------------------------------------------------
// Per-channel white levels (ZC-13)
// ---------------------------------------------------------------------------------------------

/// The chroma codes are log-spaced, exact at 1, monotone, saturate at the ends and keep the stated
/// error everywhere inside the range.
#[test]
fn chroma_codes_are_exact_at_one_and_within_the_stated_error() {
    use super::{
        CHROMA_LIMIT, CHROMA_MAX_CODE, CHROMA_MAX_REL_ERROR, CHROMA_NEUTRAL, chroma_decode,
        chroma_encode, chroma_in_range,
    };
    assert_eq!(chroma_encode(1.0), CHROMA_NEUTRAL);
    assert_eq!(chroma_decode(CHROMA_NEUTRAL), 1.0);
    let mut worst = 0.0_f64;
    for k in 0..=4000 {
        let factor =
            (1.0 / CHROMA_LIMIT) * (CHROMA_LIMIT * CHROMA_LIMIT).powf(f64::from(k) / 4000.0);
        let code = chroma_encode(factor);
        assert!(code <= CHROMA_MAX_CODE, "factor {factor}");
        worst = worst.max((chroma_decode(code) / factor - 1.0).abs());
    }
    assert!(
        worst <= CHROMA_MAX_REL_ERROR,
        "worst relative error {worst} against the stated {CHROMA_MAX_REL_ERROR}"
    );
    assert!(
        worst > 0.5 * CHROMA_MAX_REL_ERROR,
        "the bound is not absurdly loose: worst {worst}"
    );
    assert!((0..CHROMA_MAX_CODE).all(|c| chroma_decode(c) < chroma_decode(c + 1)));
    assert!(chroma_decode(0).mul_add(CHROMA_LIMIT, -1.0).abs() < 1e-12);
    assert!((chroma_decode(CHROMA_MAX_CODE) / CHROMA_LIMIT - 1.0).abs() < 1e-12);
    // Outside the range the code saturates; a dead or invalid channel gets code 0.
    assert_eq!(chroma_encode(10.0), CHROMA_MAX_CODE);
    assert_eq!(chroma_encode(0.01), 0);
    assert_eq!(chroma_encode(0.0), 0);
    assert_eq!(chroma_encode(f64::NAN), 0);
    assert!(chroma_in_range(1.5) && chroma_in_range(1.0 / 1.5));
    assert!(!chroma_in_range(1.51) && !chroma_in_range(0.66) && !chroma_in_range(f64::NAN));
}

const OBLIQUE: DVec3 = DVec3::new(-60.0, 30.0, 20.0);

/// The oblique polished cube of the other tests, lit by a panel with this white frame. The cube
/// refracts the exit rays sideways by about 2 mm (20 pixels), so the light a path finds differs
/// from the light at the pixel it started from by the panel's gradient over that shift.
fn oblique_with(frame: WhiteFrame, threads: usize) -> Fixture {
    let pose = ortho_pose(OBLIQUE);
    let mut fixture = cube_fixture(OBLIQUE, 1.5, 1.0);
    fixture.options.samples = 64;
    fixture.options.threads = threads;
    fixture.rig.lighting = RigLighting::backlight(
        vec![super::PanelGeom::facing_camera(
            &pose,
            200.0,
            [400.0, 400.0],
        )],
        vec![Some(frame)],
    );
    fixture
}

/// A panel that is redder to the right, bluer to the left and greener downwards.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the white-frame level precision; the levels are within 0.8..=1.2"
)]
fn coloured_panel(c: usize, x: f64, y: f64) -> f32 {
    match c {
        0 => (0.8 + 0.4 * x / 200.0) as f32,
        1 => (0.9 + 0.2 * y / 200.0) as f32,
        _ => (1.2 - 0.4 * x / 200.0) as f32,
    }
}

/// The relative differences of channel `c` of two predictions at the pixels usable in both.
fn channel_differences(a: &ForwardRecords, b: &ForwardRecords, alpha: f64, c: usize) -> Vec<f64> {
    let pa = evaluate(a, &|_, _| alpha);
    let pb = evaluate(b, &|_, _| alpha);
    pa[0]
        .rgb
        .iter()
        .zip(&pb[0].rgb)
        .zip(pa[0].valid.iter().zip(&pb[0].valid))
        .filter(|(_, (in_a, in_b))| **in_a && **in_b)
        .map(|((x, y), _)| f64::from(x[c]) / f64::from(y[c]) - 1.0)
        .collect()
}

/// A coloured panel gradient is reproduced per channel: channel `c` of a trace under the coloured
/// frame equals channel `c` of a trace under the grey frame that holds only channel `c`'s levels,
/// to the quantisation of the chroma codes, with and without absorption. The old grey reduction
/// (the mean of the channels) is several percent off in the red channel, so the test would fail
/// under it. The result is bitwise the same for any thread count, and survives the cache.
#[test]
fn a_coloured_panel_gradient_is_reproduced_per_channel() {
    use super::CHROMA_MAX_REL_ERROR;
    let coloured = oblique_with(graded_frame(coloured_panel), 2).trace();
    assert!(
        !coloured.views[0].chroma.is_empty(),
        "a coloured panel stores colour codes"
    );
    assert_eq!(
        coloured.views[0].chroma.len(),
        3 * coloured.views[0].record_count()
    );
    for c in 0..3 {
        let reference = oblique_with(graded_frame(|_, x, y| coloured_panel(c, x, y)), 2).trace();
        assert_eq!(reference.views[0].chroma, [] as [u8; 0]);
        for alpha in [0.0, 0.1] {
            let differences = channel_differences(&coloured, &reference, alpha, c);
            assert!(differences.len() >= 16, "{} pixels", differences.len());
            let worst = differences.iter().fold(0.0_f64, |m, d| m.max(d.abs()));
            // Half a code step in the one channel (0.16 percent) plus the f32 weights; the
            // reference's own cluster merge is the same.
            assert!(
                worst < 2.5 * CHROMA_MAX_REL_ERROR,
                "channel {c}, alpha {alpha}: worst relative difference {worst}"
            );
        }
    }

    // The grey model of the earlier version (the mean of the channels) misses the red channel by
    // percent, so the coloured frame is really tested.
    let grey_mean = oblique_with(
        graded_frame(|_, x, y| {
            (coloured_panel(0, x, y) + coloured_panel(1, x, y) + coloured_panel(2, x, y)) / 3.0
        }),
        2,
    )
    .trace();
    assert_eq!(grey_mean.views[0].chroma, [] as [u8; 0]);
    let worst_grey = channel_differences(&coloured, &grey_mean, 0.0, 0)
        .iter()
        .fold(0.0_f64, |m, d| m.max(d.abs()));
    assert!(
        worst_grey > 0.01,
        "the grey reduction would differ from the red channel by {worst_grey}"
    );

    // Determinism for 1, 4 and 16 threads, and the cache round trip with colour codes.
    for threads in [1, 4, 16] {
        let again = oblique_with(graded_frame(coloured_panel), threads).trace();
        assert_eq!(again.views, coloured.views, "{threads} threads");
    }
    let bytes = cache::encode(&coloured, 5);
    let decoded = cache::decode(&bytes, 5).expect("a valid cache file");
    assert_eq!(decoded.views, coloured.views);
    assert_ne!(decoded.views[0].chroma, [] as [u8; 0]);

    // The store checks the colour codes.
    let mut broken = coloured.views[0].clone();
    assert!(broken.check_offsets(1, 16).is_ok());
    broken.chroma.pop();
    assert!(broken.check_offsets(1, 16).is_err(), "a short chroma store");
    broken.chroma.push(255);
    assert!(broken.check_offsets(1, 16).is_err(), "a code out of range");
}

/// A grey panel (even one whose brightness varies) stores no colour codes and gives equal
/// channels, exactly the grey model; a panel whose colour varies by less than the quantisation is
/// stored as grey and agrees with it to well within that bound.
#[test]
fn a_grey_panel_is_unchanged_and_a_nearly_grey_one_stays_within_the_quantisation() {
    use super::CHROMA_MAX_REL_ERROR;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the white-frame level precision; the levels are within 0.8..=1.2"
    )]
    let brightness = |x: f64| (0.8 + 0.4 * x / 200.0) as f32;
    let grey = oblique_with(graded_frame(|_, x, _| brightness(x)), 2).trace();
    assert_eq!(grey.views[0].chroma, [] as [u8; 0]);
    let prediction = evaluate(&grey, &|_, _| 0.1);
    let mut seen = 0;
    for (rgb, ok) in prediction[0].rgb.iter().zip(&prediction[0].valid) {
        if *ok {
            #[expect(
                clippy::float_cmp,
                reason = "a grey panel must give bit-identical channels (the grey model)"
            )]
            let grey_pixel = rgb[0] == rgb[1] && rgb[1] == rgb[2];
            assert!(grey_pixel, "{rgb:?}");
            seen += 1;
        }
    }
    assert!(seen >= 16);

    // Colour that changes by 0.1 percent across the photo: over the 20 pixel shift of the exit
    // light that is 0.01 percent, far below half a code step.
    let delta = [0.001_f64, 0.0, -0.001];
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the white-frame level precision; the levels are within 0.8..=1.2"
    )]
    let nearly = oblique_with(
        graded_frame(|c, x, _| (f64::from(brightness(x)) * (1.0 + delta[c] * x / 200.0)) as f32),
        2,
    )
    .trace();
    assert!(
        nearly.views[0].chroma.is_empty(),
        "below the quantisation the view is stored as grey"
    );
    for c in 0..3 {
        for alpha in [0.0, 0.1] {
            let worst = channel_differences(&nearly, &grey, alpha, c)
                .iter()
                .fold(0.0_f64, |m, d| m.max(d.abs()));
            assert!(
                worst < CHROMA_MAX_REL_ERROR,
                "channel {c}, alpha {alpha}: {worst}"
            );
        }
    }
}

/// Light whose colour differs from the camera ray's own pixel by more than the stored range is
/// flagged like light from an unseen place, so the pixel is dropped instead of silently biased.
#[test]
fn light_beyond_the_chroma_range_is_flagged() {
    // Red is 1 in a band around the centre of the photo (the camera rays start at pixels 96 to
    // 104) and 0.1 outside it; the cube shifts the exit light by about 20 pixels, out of the band,
    // where red is a tenth of its own value while green and blue are unchanged: the red factor
    // against the channel mean is 0.14.
    let step = |c: usize, x: f64, _y: f64| -> f32 {
        if c == 0 && !(92.0..108.0).contains(&x) {
            0.1
        } else {
            1.0
        }
    };
    let abrupt = oblique_with(graded_frame(step), 2).trace();
    let flat = oblique_with(graded_frame(|_, _, _| 1.0), 2).trace();
    let view = &abrupt.views[0];
    assert!(
        (0..view.pixel_count()).any(|p| view.status[p] & super::status::FLAGGED_LIGHT != 0),
        "some pixel is dropped for its out-of-range colour"
    );
    assert!(abrupt.valid_pixels() < flat.valid_pixels());
}

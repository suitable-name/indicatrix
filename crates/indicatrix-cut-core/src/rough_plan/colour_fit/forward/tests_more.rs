//! More tests of the forward tracer (written, not run, by lane F1): the disk cache, record
//! compression, the white furnace, the Jacobian, the light model and small utilities.

use std::sync::atomic::AtomicBool;

use glam::DVec3;

use super::{
    CacheStatus, ForwardInput, PathRecord, RigLighting, SpectralModel, StoneIndex, SurfaceClass,
    SurfaceMap, WhiteFrame, cache,
    compress::compress,
    evaluate, evaluate_with_jacobian, for_each_pixel_jacobian,
    parallel::{run_indexed, thread_count},
    rng::{PixelSequence, Rng},
    tests::{bicolour, centre_grid, colour_rig, cube_fixture, mean_channel, ortho_pose},
    trace_rig,
};
use crate::rough_plan::{
    camera_spectral::{CameraResponse, GRID_LEN, ResponseTier},
    locate::{RigProfile, box_mesh},
    photometry::LinearImage,
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
    };
    let mut raw = vec![record; 40];
    let mut out = Vec::new();
    assert_eq!(compress(&mut raw, 16, 0.3, 1, &mut out), 0.0);
    assert_eq!(out.len(), 1);
    assert!((out[0].weight - 10.0).abs() < 1e-5);
    assert!((out[0].lengths[0] - 3.5).abs() < 1e-6);
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
    assert!((frame.level_at(1.5, 2.5).unwrap() - 2.0).abs() < 1e-6);
    assert!((frame.level_at(2.0, 2.5).unwrap() - 2.5).abs() < 1e-6);
    assert!(frame.level_at(-0.1, 1.0).is_none());
    assert!(frame.level_at(4.0, 1.0).is_none());

    // A rig whose image is the 200 x 200 default: a uniform panel level 2.
    let pose = ortho_pose(DVec3::new(-100.0, 0.0, 0.0));
    let uniform =
        WhiteFrame::from_levels([200, 200], 1.0, 200, 200, vec![2.0; 200 * 200]).expect("a frame");
    let lighting = RigLighting::backlight(
        vec![super::PanelGeom::facing_camera(
            &pose,
            200.0,
            [400.0, 400.0],
        )],
        vec![Some(uniform)],
    );
    let rig = RigProfile::new("rig", vec![pose], 1.5);
    let own = 2.0;
    // Through the middle: the camera saw that panel point, so the ratio is 1.
    let seen = lighting.exit_light(&rig, 0, DVec3::ZERO, DVec3::X, own);
    assert!((seen.ratio - 1.0).abs() < 1e-9 && !seen.flagged);
    // Sideways by 40 mm: that panel point projects outside the 20 mm wide image: flagged mean.
    let unseen = lighting.exit_light(&rig, 0, DVec3::new(0.0, 40.0, 0.0), DVec3::X, own);
    assert!((unseen.ratio - 1.0).abs() < 1e-9 && unseen.flagged);
    // Away from the panel and past its edge: dark.
    assert_eq!(
        lighting
            .exit_light(&rig, 0, DVec3::ZERO, -DVec3::X, own)
            .ratio,
        0.0
    );
    assert_eq!(
        lighting
            .exit_light(&rig, 0, DVec3::new(0.0, 500.0, 0.0), DVec3::X, own)
            .ratio,
        0.0
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
    let tiny = WhiteFrame::from_levels([50, 50], 1.0, 50, 50, vec![1.0; 50 * 50]).expect("a frame");
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
        cells[(px * 4.0) as usize * 4 + (py * 4.0) as usize] = true;
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

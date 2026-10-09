//! Tests of the forward tracer (written, not run, by lane F1): analytic slab, immersion,
//! holder, zone lengths and determinism.
//!
//! The cache, compression, furnace, Jacobian and light
//! tests are in `tests_more`.

use std::sync::atomic::AtomicBool;

use glam::DVec3;
use indicatrix::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption, zone_lengths},
};

use super::{
    ColourRig, ForwardInput, ForwardOptions, ForwardRecords, PanelGeom, RigLighting, StoneIndex,
    SurfaceMap, ViewTraceInput, evaluate, status, trace_rig,
};
use crate::rough_plan::{
    camera_spectral::{BacklightSpectrum, CameraResponse, GRID_LEN, ResponseTier},
    locate::{InclusionShell, Projection, RigProfile, Rigid, ViewPose, box_mesh},
    photometry::WorkingGrid,
    shape::RoughMesh,
};

/// Everything a test scene owns.
pub(super) struct Fixture {
    pub mesh: RoughMesh,
    pub rig: ColourRig,
    pub surfaces: SurfaceMap,
    pub index: StoneIndex,
    pub zones: Option<ZonedAbsorption>,
    pub inclusions: Vec<InclusionShell>,
    pub camera: CameraResponse,
    pub backlight: BacklightSpectrum,
    pub options: ForwardOptions,
    pub views: Vec<ViewTraceInput<'static>>,
}

impl Fixture {
    pub(super) fn input(&self) -> ForwardInput<'_> {
        ForwardInput {
            mesh: &self.mesh,
            alignment: Rigid::IDENTITY,
            rig: &self.rig,
            surfaces: &self.surfaces,
            index: &self.index,
            zones: self.zones.as_ref(),
            inclusions: &self.inclusions,
            camera: &self.camera,
            backlight: &self.backlight,
            views: &self.views,
            options: &self.options,
            cache_dir: None,
        }
    }

    pub(super) fn trace(&self) -> ForwardRecords {
        trace_rig(&self.input(), &AtomicBool::new(false), &mut |_| {}).expect("the trace runs")
    }
}

/// An orthographic camera at `position` looking at the origin (10 pixels per mm, 200 x 200).
pub(super) fn ortho_pose(position: DVec3) -> ViewPose {
    ViewPose::look_at(
        "test",
        position,
        DVec3::ZERO,
        DVec3::Z,
        Projection::Orthographic { px_per_mm: 10.0 },
        [200, 200],
    )
}

/// An 8 x 8 working grid of one-pixel cells around the principal point.
pub(super) fn centre_grid() -> WorkingGrid {
    WorkingGrid::fit([96, 96, 104, 104], 8).expect("a valid region")
}

pub(super) fn flat_camera() -> CameraResponse {
    CameraResponse::from_grid([[1.0; GRID_LEN]; 3], ResponseTier::Measured).expect("finite")
}

pub(super) fn flat_backlight() -> BacklightSpectrum {
    BacklightSpectrum::from_grid(&[1.0; GRID_LEN]).expect("positive")
}

pub(super) fn backlit(pose: &ViewPose) -> RigLighting {
    RigLighting::backlight(
        vec![PanelGeom::facing_camera(pose, 200.0, [400.0, 400.0])],
        vec![None],
    )
}

pub(super) fn colour_rig(
    pose: &ViewPose,
    stone_n: f64,
    surround_n: f64,
    lighting: RigLighting,
) -> ColourRig {
    let mut profile = RigProfile::new("test rig", vec![pose.clone()], stone_n);
    profile.surround_n = surround_n;
    ColourRig::new(profile, lighting)
}

/// A 10 mm cube seen by one orthographic camera at `position`, polished, backlit, flat spectra.
pub(super) fn cube_fixture(position: DVec3, stone_n: f64, surround_n: f64) -> Fixture {
    let pose = ortho_pose(position);
    let lighting = backlit(&pose);
    let options = ForwardOptions {
        samples: 128,
        bins: 4,
        max_sample_factor: 1,
        threads: 2,
        ..ForwardOptions::default()
    };
    Fixture {
        mesh: box_mesh(DVec3::splat(5.0)).expect("a cube"),
        rig: colour_rig(&pose, stone_n, surround_n, lighting),
        surfaces: SurfaceMap::polished(),
        index: StoneIndex::Rig,
        zones: None,
        inclusions: Vec::new(),
        camera: flat_camera(),
        backlight: flat_backlight(),
        options,
        views: vec![ViewTraceInput::new(0, centre_grid())],
    }
}

/// A zone absorption with one Gaussian band; only the geometry matters in these tests.
pub(super) fn absorb(peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 1.0e6, peak,
    )]))
}

/// Base zone plus one half space `x >= 0`.
pub(super) fn bicolour() -> ZonedAbsorption {
    let mut zoned = ZonedAbsorption::new(absorb(0.1));
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        absorption: absorb(0.3),
    });
    zoned
}

/// The mean over the valid pixels of channel `c` of the first view's prediction.
pub(super) fn mean_channel(records: &ForwardRecords, alpha: f64, c: usize) -> f64 {
    let predictions = evaluate(records, &|_, _| alpha);
    let view = &predictions[0];
    let values: Vec<f64> = view
        .rgb
        .iter()
        .zip(&view.valid)
        .filter(|(_, ok)| **ok)
        .map(|(rgb, _)| f64::from(rgb[c]))
        .collect();
    assert_ne!(values, [] as [f64; 0], "some pixel must be valid");
    values.iter().sum::<f64>() / values.len() as f64
}

#[test]
fn slab_transmittance_matches_the_analytic_formula() {
    let mut fixture = cube_fixture(DVec3::new(-100.0, 0.0, 0.0), 1.5, 1.0);
    fixture.options.samples = 512;
    let records = fixture.trace();
    assert_eq!(
        records.spectral.traced_bins, 1,
        "no dispersion: one shared bin"
    );
    let (alpha, thickness) = (0.2_f64, 10.0_f64);
    let reflectance = ((1.5_f64 - 1.0) / (1.5 + 1.0)).powi(2);
    let attenuation = (-alpha * thickness).exp();
    let expected = (1.0 - reflectance).powi(2) * attenuation
        / (reflectance * reflectance * attenuation).mul_add(-attenuation, 1.0);
    for c in 0..3 {
        let measured = mean_channel(&records, alpha, c);
        assert!(
            (measured / expected - 1.0).abs() < 0.01,
            "channel {c}: measured {measured}, expected {expected}"
        );
    }
    // Without absorption: (1 - R) / (1 + R).
    let clear = (1.0 - reflectance) / (1.0 + reflectance);
    assert!((mean_channel(&records, 0.0, 1) / clear - 1.0).abs() < 0.01);
}

#[test]
fn immersion_with_equal_indices_gives_straight_rays() {
    let position = DVec3::new(-60.0, 30.0, 20.0);
    let fixture = cube_fixture(position, 1.5, 1.5);
    let records = fixture.trace();
    let view = &records.views[0];
    let pose = ortho_pose(position);
    let (origin, dir) = pose.pixel_ray(glam::DVec2::new(100.0, 100.0));
    let (enter, exit) = super::box_chord(origin, dir, DVec3::splat(-5.0), DVec3::splat(5.0))
        .expect("the central ray crosses the cube");
    let chord = exit - enter;
    let mut seen = 0;
    for p in 0..view.pixel_count() {
        if !view.is_valid(p) {
            continue;
        }
        let list = view.pixel_records(p, 0, records.spectral.traced_bins);
        assert_eq!(list.len(), 1, "a straight pixel is one record");
        assert!(
            (f64::from(list[0].lengths[0]) - chord).abs() < 2e-3,
            "length {} against the chord {chord}",
            list[0].lengths[0]
        );
        assert!((list[0].weight - 1.0).abs() < 1e-4, "no reflection loss");
        seen += 1;
    }
    assert!(seen >= 32, "most of the 8 x 8 pixels see the stone");
}

#[test]
fn the_holder_occludes_the_light() {
    let position = DVec3::new(-100.0, 0.0, 0.0);
    let open = cube_fixture(position, 1.5, 1.0);
    let open_records = open.trace();
    assert!(mean_channel(&open_records, 0.0, 0) > 0.8);

    let mut blocked = cube_fixture(position, 1.5, 1.0);
    let holder = box_mesh(DVec3::new(2.5, 3.0, 3.0))
        .expect("a box")
        .translated(DVec3::new(22.5, 0.0, 0.0))
        .expect("a moved box");
    blocked.rig.lighting.holder = Some(holder);
    let records = blocked.trace();
    let view = &records.views[0];
    for p in 0..view.pixel_count() {
        assert!(
            view.pixel_records(p, 0, records.spectral.traced_bins)
                .is_empty(),
            "pixel {p} sees the holder, not the panel"
        );
    }
    let predictions = evaluate(&records, &|_, _| 0.0);
    assert!(
        predictions[0]
            .rgb
            .iter()
            .all(|rgb| rgb.iter().all(|&v| v == 0.0))
    );
}

#[test]
fn zone_lengths_in_records_equal_the_kernel_for_a_straight_path() {
    let position = DVec3::new(-100.0, 0.0, 0.0);
    let mut fixture = cube_fixture(position, 1.5, 1.5);
    let zoned = bicolour();
    fixture.zones = Some(zoned.clone());
    let records = fixture.trace();
    assert_eq!(records.n_zones, 2);
    let view = &records.views[0];
    let pose = ortho_pose(position);
    let mut checked = 0;
    for p in 0..view.pixel_count() {
        if !view.is_valid(p) {
            continue;
        }
        let (x, y) = (p % view.grid.width, p / view.grid.width);
        let (origin, dir) = pose.pixel_ray(view.grid.centre(x, y));
        // The ray is along +x; the cube spans x in -5..5.
        let entry = origin + dir * 95.0;
        let exit = origin + dir * 105.0;
        let expected = zone_lengths(&zoned, entry, exit);
        let list = view.pixel_records(p, 0, records.spectral.traced_bins);
        assert_eq!(list.len(), 1);
        for (z, &expected_length) in expected.iter().take(2).enumerate() {
            assert!(
                (f64::from(list[0].lengths[z]) - expected_length).abs() < 2e-3,
                "pixel {p} zone {z}: {} against {}",
                list[0].lengths[z],
                expected_length
            );
        }
        checked += 1;
    }
    assert!(checked >= 32);
}

#[test]
fn records_are_bitwise_identical_for_any_thread_count() {
    let position = DVec3::new(-60.0, 30.0, 20.0);
    let mut fixture = cube_fixture(position, 1.6, 1.0);
    fixture.surfaces = SurfaceMap::frosted(0.15);
    fixture.index = StoneIndex::Dispersion(DispersionModel::Sellmeier1 { b1: 1.2, c1: 0.01 });
    fixture.options.samples = 24;
    fixture.options.max_sample_factor = 2;
    fixture.zones = Some(bicolour());

    let mut baseline: Option<ForwardRecords> = None;
    for (threads, chunk) in [(1, 64), (4, 3), (16, 5), (16, 1)] {
        fixture.options.threads = threads;
        fixture.options.chunk_pixels = chunk;
        let records = fixture.trace();
        assert_eq!(records.spectral.traced_bins, 4);
        match &baseline {
            None => baseline = Some(records),
            Some(first) => assert_eq!(
                first, &records,
                "{threads} threads with chunks of {chunk} differ"
            ),
        }
    }
}

#[test]
fn an_inclusion_removes_its_paths_and_drops_blocked_pixels() {
    let position = DVec3::new(-100.0, 0.0, 0.0);
    let mut fixture = cube_fixture(position, 1.5, 1.0);
    // A 20 x 20 grid of one-pixel cells: +-1 mm around the axis.
    fixture.views = vec![ViewTraceInput::new(
        0,
        WorkingGrid::fit([90, 90, 110, 110], 20).expect("a valid region"),
    )];
    let (points, triangles) = crate::rough_plan::locate::sphere_shell(DVec3::ZERO, 0.5);
    fixture.inclusions = vec![InclusionShell {
        points,
        triangles,
        margin_mm: 0.3,
    }];
    let records = fixture.trace();
    let view = &records.views[0];
    let width = view.grid.width;
    let centre = 10 * width + 10;
    assert_ne!(
        view.status[centre] & status::INCLUSION,
        0,
        "the axis pixel is blocked"
    );
    assert!(!view.is_valid(centre));
    assert!(
        view.is_valid(0),
        "a corner pixel, 1.4 mm from the axis, is clear"
    );
    assert!(view.loss[centre].inclusion > 0.9);
}

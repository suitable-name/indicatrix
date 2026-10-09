//! Tests of the zone suggestion and of the joint refinement against the real forward tracer and
//! solver (written, not run, by lane G1).
//!
//! The photos are made from the tracer's own records, so
//! the physics agrees with the fit by construction; the fixture is the one of the solver's tests
//! (a 10 mm cube, orthographic cameras, polished surface).

use std::sync::atomic::AtomicBool;

use glam::DVec3;
use indicatrix::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
};

use super::{
    NO_LABEL, RefineOptions, SuggestError, SuggestKind, SuggestOptions, SuggestView, ZoneLocks,
    ZoneParameter, parameter_value, refine_zone_geometry, segment_views, suggest_zones,
};
use crate::rough_plan::{
    camera_spectral::{BacklightSpectrum, CameraResponse},
    colour_fit::{
        ColourRig,
        forward::{
            ForwardInput, ForwardOptions, ForwardRecords, PanelGeom, RigLighting, SpectralModel,
            StoneIndex, SurfaceMap, ViewTraceInput, evaluate, trace_rig,
        },
        solve::{FitConfig, FitInputs, ObservedView, SmoothBasisModel},
    },
    locate::{Projection, RigProfile, Rigid, Scene, ViewPose, box_mesh},
    photometry::WorkingGrid,
    shape::RoughMesh,
};

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

fn ortho_pose(position: DVec3) -> ViewPose {
    ViewPose::look_at(
        "test",
        position,
        DVec3::ZERO,
        DVec3::Z,
        Projection::Orthographic { px_per_mm: 10.0 },
        [200, 200],
    )
}

/// A square working grid over the central 8 mm of the image with at most `max_px` pixels a side.
fn window_grid(max_px: usize) -> WorkingGrid {
    WorkingGrid::fit([60, 60, 140, 140], max_px).expect("a valid region")
}

struct Fixture {
    mesh: RoughMesh,
    rig: ColourRig,
    surfaces: SurfaceMap,
    index: StoneIndex,
    zones: Option<ZonedAbsorption>,
    camera: CameraResponse,
    backlight: BacklightSpectrum,
    options: ForwardOptions,
    views: Vec<ViewTraceInput<'static>>,
}

impl Fixture {
    fn new(positions: &[(f64, f64, f64)], max_px: usize, zones: Option<ZonedAbsorption>) -> Self {
        let poses: Vec<ViewPose> = positions
            .iter()
            .map(|&(x, y, z)| ortho_pose(DVec3::new(x, y, z)))
            .collect();
        let panels: Vec<PanelGeom> = poses
            .iter()
            .map(|pose| PanelGeom::facing_camera(pose, 200.0, [400.0, 400.0]))
            .collect();
        let lighting = RigLighting::backlight(panels, positions.iter().map(|_| None).collect());
        let mut profile = RigProfile::new("test rig", poses, 1.6);
        profile.surround_n = 1.0;
        Self {
            mesh: box_mesh(DVec3::splat(5.0)).expect("a cube"),
            rig: ColourRig::new(profile, lighting),
            surfaces: SurfaceMap::polished(),
            index: StoneIndex::Rig,
            zones,
            camera: CameraResponse::from_srgb().expect("the sRGB fallback"),
            backlight: BacklightSpectrum::from_cct_k(5000.0).expect("an LED spectrum"),
            options: ForwardOptions {
                samples: 64,
                max_sample_factor: 1,
                threads: 2,
                ..ForwardOptions::default()
            },
            views: (0..positions.len())
                .map(|v| ViewTraceInput::new(v, window_grid(max_px)))
                .collect(),
        }
    }

    fn input(&self) -> ForwardInput<'_> {
        ForwardInput {
            mesh: &self.mesh,
            alignment: Rigid::IDENTITY,
            rig: &self.rig,
            surfaces: &self.surfaces,
            index: &self.index,
            zones: self.zones.as_ref(),
            inclusions: &[],
            camera: &self.camera,
            backlight: &self.backlight,
            views: &self.views,
            options: &self.options,
            cache_dir: None,
        }
    }

    fn trace(&self) -> ForwardRecords {
        trace_rig(&self.input(), &AtomicBool::new(false), &mut |_| {}).expect("the trace runs")
    }

    fn scene(&self) -> Scene<'_> {
        Scene::new(&self.mesh, &self.rig.rig, Rigid::IDENTITY)
    }
}

fn band(peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 40.0, peak,
    )]))
}

/// The base zone plus the half space `x >= offset`.
fn bicolour(offset: f64) -> ZonedAbsorption {
    let mut zoned = ZonedAbsorption::new(band(0.1));
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset,
        },
        absorption: band(0.3),
    });
    zoned
}

/// `SplitMix64` with Box-Muller normals: the seeded noise of the synthetic photos.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// Photos of `records` for the absorption `alpha(zone, lambda)`, the per-view `gains` and the
/// noise standard deviation `sigma` (also the stated variance).
fn synthesize(
    records: &ForwardRecords,
    alpha: &dyn Fn(usize, f64) -> f64,
    gains: &[f64],
    sigma: f64,
    seed: u64,
) -> Vec<ObservedView> {
    evaluate(records, alpha)
        .iter()
        .enumerate()
        .map(|(slot, prediction)| {
            let mut noise = Noise(seed.wrapping_mul(0x1000_0001).wrapping_add(slot as u64 + 1));
            let n = prediction.rgb.len();
            let mut values = vec![[0.0_f32; 3]; n];
            for p in 0..n {
                if !prediction.valid[p] {
                    continue;
                }
                for c in 0..3 {
                    let clean = gains[slot] * f64::from(prediction.rgb[p][c]);
                    values[p][c] = (clean + sigma * noise.gauss()) as f32;
                }
            }
            ObservedView {
                view: prediction.view,
                width: prediction.grid.width,
                height: prediction.grid.height,
                values,
                variance: vec![[(sigma * sigma) as f32; 3]; n],
            }
        })
        .collect()
}

fn log_linear(a0: f64, slope: f64) -> Vec<f64> {
    (0..7)
        .map(|k| a0.ln() + slope * (f64::from(k) - 3.0))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Segmentation and suggestion
// ---------------------------------------------------------------------------------------------

fn two_colour_view() -> SuggestView {
    let grid = WorkingGrid {
        width: 10,
        height: 10,
        origin: [0.0, 0.0],
        scale: 1.0,
    };
    let mut values = Vec::new();
    for y in 0..10 {
        for x in 0..10 {
            let jitter = 0.002 * ((x * 7 + y * 3) % 5) as f32;
            values.push(if x < 5 {
                [0.8 + jitter, 0.8, 0.8 - jitter]
            } else {
                [0.2 + jitter, 0.3, 0.1]
            });
        }
    }
    SuggestView {
        view: 0,
        grid,
        values,
        valid: vec![true; 100],
        kind: SuggestKind::Transmittance,
    }
}

#[test]
fn two_colours_are_segmented_with_the_darker_cluster_first() {
    let segmentation =
        segment_views(&[two_colour_view()], &SuggestOptions::default()).expect("a segmentation");
    assert_eq!(segmentation.k, 2);
    let labels = &segmentation.views[0].labels;
    for y in 0..10 {
        for x in 0..10 {
            let expected = u8::from(x < 5);
            assert_eq!(labels[y * 10 + x], expected, "pixel ({x}, {y})");
        }
    }
    assert!(segmentation.centres[0][0] < segmentation.centres[1][0]);
    assert_eq!(segmentation.inertia.len(), 3, "k = 2, 3 and 4 were tried");
}

#[test]
fn unused_pixels_keep_no_label_and_bad_input_is_refused() {
    let mut view = two_colour_view();
    view.valid[0] = false;
    let segmentation = segment_views(&[view], &SuggestOptions::default()).expect("a segmentation");
    assert_eq!(segmentation.views[0].labels[0], NO_LABEL);

    let bad_k = SuggestOptions {
        k: Some(1),
        ..SuggestOptions::default()
    };
    assert!(matches!(
        segment_views(&[two_colour_view()], &bad_k),
        Err(SuggestError::BadOptions(_))
    ));
    assert!(matches!(
        segment_views(&[], &SuggestOptions::default()),
        Err(SuggestError::NoPixels)
    ));
    let mut short = two_colour_view();
    short.valid.pop();
    assert!(matches!(
        segment_views(&[short], &SuggestOptions::default()),
        Err(SuggestError::BadView(0))
    ));
    let mut residual = two_colour_view();
    residual.kind = SuggestKind::Residual;
    assert!(matches!(
        segment_views(&[two_colour_view(), residual], &SuggestOptions::default()),
        Err(SuggestError::MixedKinds)
    ));
}

#[test]
fn a_bicolour_boundary_is_found_in_synthetic_forward_data() {
    // Three cameras whose viewing directions lie in the boundary plane x = 0: the colour step in
    // their photos is exactly at the surface trace of the plane (see the module docs of
    // `suggest` for why oblique views are biased).
    let positions = [(0.0, -100.0, 0.0), (0.0, -80.0, 60.0), (0.0, 80.0, -60.0)];
    let fixture = Fixture::new(&positions, 10, Some(bicolour(0.0)));
    let records = fixture.trace();
    let alpha = |zone: usize, lambda: f64| {
        if zone == 0 {
            0.002
        } else {
            0.02 + 0.25 * (-((lambda - 570.0) / 70.0).powi(2)).exp()
        }
    };
    let views: Vec<SuggestView> = evaluate(&records, &alpha)
        .iter()
        .map(SuggestView::from_prediction)
        .collect();
    let options = SuggestOptions {
        k: Some(2),
        ..SuggestOptions::default()
    };
    let report = suggest_zones(&fixture.scene(), &views, &options).expect("a report");

    // Each photo is split in two uniform halves at the boundary columns.
    assert_eq!(report.segmentation.k, 2);
    for view in &report.segmentation.views {
        let side = |columns: std::ops::Range<usize>| -> Vec<u8> {
            let mut set: Vec<u8> = columns
                .flat_map(|x| (0..view.height).map(move |y| (x, y)))
                .map(|(x, y)| view.labels[y * view.width + x])
                .filter(|l| *l != NO_LABEL)
                .collect();
            set.sort_unstable();
            set.dedup();
            set
        };
        let (left, right) = (side(0..5), side(5..10));
        assert_eq!(left.len(), 1, "view {}: left half {left:?}", view.view);
        assert_eq!(right.len(), 1, "view {}: right half {right:?}", view.view);
        assert_ne!(left, right);
    }

    let best = report.suggestions.first().expect("a suggestion");
    match &best.shape {
        ZoneShape::HalfSpace { normal, offset } => {
            assert!(normal.dot(DVec3::X) > 0.95, "normal {normal:?}");
            assert!(offset.abs() < 0.2, "offset {offset}");
        }
        other => panic!("not a half space: {other:?}"),
    }
    assert!(best.score > 0.5, "score {}", best.score);
    assert!(
        best.views_supporting >= 2,
        "views {}",
        best.views_supporting
    );
    assert_eq!(best.target_label, 0);
    for pair in report.suggestions.windows(2) {
        assert!(pair[0].score >= pair[1].score, "sorted by score");
    }
}

// ---------------------------------------------------------------------------------------------
// Refinement against the photos
// ---------------------------------------------------------------------------------------------

#[test]
fn the_refinement_moves_a_boundary_offset_of_0_3_mm_to_within_0_1_mm() {
    let positions = [
        (0.0, -100.0, 0.0),
        (-70.0, -70.0, 30.0),
        (70.0, -70.0, -30.0),
        (0.0, -80.0, 60.0),
    ];
    // The photos show the boundary at x = 0.3; the refinement starts at x = 0.
    let fixture = Fixture::new(&positions, 40, Some(bicolour(0.3)));
    let records = fixture.trace();
    let model = SmoothBasisModel::new(2);
    let mut truth = log_linear(0.03, 0.2);
    truth.extend(log_linear(0.12, -0.2));
    let alpha = |zone: usize, lambda: f64| model.alpha(zone, lambda, &truth);
    let photos = synthesize(&records, &alpha, &[1.0; 4], 0.002, 3);
    let config = FitConfig {
        seeds: 4,
        stage1_iterations: 10,
        finalists: 1,
        smoothness_sigma: 0.25,
        threads: 2,
        ..FitConfig::default()
    };
    let inputs = FitInputs {
        forward: fixture.input(),
        observed: &photos,
        config: &config,
        surfaces_for_roughness: None,
    };
    let result = refine_zone_geometry(
        &inputs,
        &bicolour(0.0),
        &ZoneLocks::new(),
        &RefineOptions {
            step_mm: 0.1,
            ..RefineOptions::default()
        },
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .expect("the refinement runs");
    let offset =
        parameter_value(&result.zoned.zones[0].shape, ZoneParameter::Offset).expect("an offset");
    assert!((offset - 0.3).abs() < 0.1, "refined offset {offset}");
    assert!(result.chi2_after <= result.chi2_before);
    assert!(result.fit.is_some());
}

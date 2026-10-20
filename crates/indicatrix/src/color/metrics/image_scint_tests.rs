use super::*;
use crate::{
    color::metrics::{camera_view_basis, evaluate_optical_image},
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
};
use std::cell::Cell;

const POSE: [f32; 3] = [0.6, 0.45, 2.4];

/// Evaluates the round brilliant under `LightTent` at the shared pose.
fn evaluate(
    want_fire: bool,
    want_scintillation: bool,
    pose: Option<[f32; 3]>,
    cancelled: impl Fn() -> bool,
) -> Result<OpticalImage, OpticalImageError> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(POSE[0], POSE[1], POSE[2], 42.0);
    let request = OpticalImageRequest {
        camera: &camera,
        width: 30,
        height: 19,
        samples: 2,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::LightTent.studio(1.0, 0.85, 0.95),
        want_fire,
        want_scintillation,
        pose,
    };
    evaluate_optical_image(&request, cancelled)
}

/// A map of `size` x `size` hit pixels whose brilliance fraction is given by `fraction`.
fn map_from(size: u32, fraction: impl Fn(u32, u32) -> bool) -> OpticalImage {
    let mut pixels = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let brilliant = fraction(x, y);
            pixels.push(OpticalPixel {
                hits: 1,
                classes: if brilliant {
                    [0, 0, 1, 0]
                } else {
                    [0, 1, 0, 0]
                },
                ..OpticalPixel::default()
            });
        }
    }
    OpticalImage {
        width: size,
        height: size,
        samples: 1,
        fire_computed: false,
        scint_computed: false,
        origin: [0, 0],
        pixels,
        total: OpticalPixel::default(),
    }
}

#[test]
fn scintillation_off_leaves_every_existing_field_unchanged() {
    let off = evaluate(true, false, Some(POSE), || false).unwrap();
    let on = evaluate(true, true, Some(POSE), || false).unwrap();
    assert!(!off.scint_computed);
    assert!(on.scint_computed);
    assert!(
        off.pixels
            .iter()
            .all(|p| p.scint_poses == 0 && p.scint_returned == 0)
    );
    assert_eq!(off.pixels.len(), on.pixels.len());
    for (a, b) in off.pixels.iter().zip(&on.pixels) {
        assert_eq!(a.hits, b.hits);
        assert_eq!(a.classes, b.classes);
        assert_eq!(a.fire_sum_deg.to_bits(), b.fire_sum_deg.to_bits());
        assert_eq!(a.fire_qualified, b.fire_qualified);
        assert_eq!(a.fire_rejected, b.fire_rejected);
        assert_eq!(a.centre.entry_facet, b.centre.entry_facet);
        assert_eq!(a.centre.exit_facet, b.centre.exit_facet);
        assert_eq!(a.centre.bounces, b.centre.bounces);
        assert_eq!(a.centre.reason, b.centre.reason);
        assert_eq!(a.mixed_facets, b.mixed_facets);
    }
    assert_eq!(off.total.classes, on.total.classes);
    assert_eq!(
        off.total.fire_sum_deg.to_bits(),
        on.total.fire_sum_deg.to_bits()
    );
    // A pose without the request changes nothing either.
    let pose_only = evaluate(true, false, None, || false).unwrap();
    assert_eq!(pose_only.total.classes, off.total.classes);
}

#[test]
fn offset_poses_are_the_grid_offsets() {
    let cameras = temporal_cameras(POSE, 0.5);
    assert_eq!(cameras.len(), SCINT_TEMPORAL_YAW_OFFSETS_DEG.len());
    assert_eq!(
        SCINT_TEMPORAL_YAW_OFFSETS_DEG[BASE_POSE].to_bits(),
        0.0f32.to_bits()
    );
    assert_eq!(usize::from(POSES), cameras.len());
    for (camera, offset_deg) in cameras.iter().zip(SCINT_TEMPORAL_YAW_OFFSETS_DEG) {
        let yaw = POSE[0] + offset_deg.to_radians();
        let expected = Camera::new(yaw, POSE[1], POSE[2], 42.0);
        assert_eq!(
            camera.origin.to_array().map(f32::to_bits),
            expected.origin.to_array().map(f32::to_bits)
        );
        assert_eq!(
            camera.forward.to_array().map(f32::to_bits),
            expected.forward.to_array().map(f32::to_bits)
        );
        assert_eq!(
            camera.right.to_array().map(f32::to_bits),
            expected.right.to_array().map(f32::to_bits)
        );
        assert_eq!(
            camera.up.to_array().map(f32::to_bits),
            expected.up.to_array().map(f32::to_bits)
        );
        assert_eq!(
            camera.fov_tan.to_bits(),
            0.5f32.to_bits(),
            "the request's field of view"
        );
        // And the grid's own basis for the same yaw points the same way.
        let (forward, right, up) = camera_view_basis(yaw, POSE[1]);
        assert!((camera.forward - forward).length() < 1e-5);
        assert!((camera.right - right).length() < 1e-5);
        assert!((camera.up - up).length() < 1e-5);
    }
}

#[test]
fn temporal_variance_is_bernoulli_and_capped() {
    let image = evaluate(false, true, Some(POSE), || false).unwrap();
    let mut computed = 0;
    let mut flickering = 0;
    for (i, pixel) in image.pixels.iter().enumerate() {
        assert!(pixel.scint_poses == 0 || pixel.scint_poses == 5, "{i}");
        assert!(pixel.scint_returned <= pixel.scint_poses);
        match image.temporal_variance(i) {
            None => assert_eq!(pixel.scint_poses, 0),
            Some(v) => {
                computed += 1;
                assert!(pixel.hits > 0, "a pixel without hits has no temporal term");
                let allowed = [0.0_f32, 0.16, 0.24];
                assert!(allowed.iter().any(|a| (a - v).abs() < 1e-6), "{v}");
                flickering += usize::from(v > 0.0);
            }
        }
    }
    assert!(
        computed > 50,
        "the stone must cover part of the frame: {computed}"
    );
    assert!(flickering > 0, "some pixel must flip within +/-3 degrees");
    assert!(image.temporal_variance(image.pixels.len()).is_none());
}

#[test]
fn spatial_term_is_zero_on_a_uniform_map_and_positive_on_a_checkerboard() {
    let uniform = map_from(9, |_, _| true);
    assert!(
        scintillation_spatial(&uniform)
            .iter()
            .all(|s| s.abs() < 1e-3)
    );
    let checker = map_from(9, |x, y| (x + y) % 2 == 0);
    let spatial = scintillation_spatial(&checker);
    assert!(spatial.iter().all(|s| (0.0..=100.0).contains(s)));
    assert!(spatial.iter().all(|s| *s > 10.0), "{spatial:?}");
    // The single-pixel read equals the whole pass, bit for bit, at the corner and inside.
    for i in [0, 8, 40, 72, 80] {
        assert_eq!(
            scintillation_spatial_at(&checker, i).to_bits(),
            spatial[i].to_bits()
        );
    }
    // A pixel with no hits reads zero in both.
    let mut hole = map_from(5, |x, y| (x + y) % 2 == 0);
    hole.pixels[12] = OpticalPixel::default();
    assert_eq!(scintillation_spatial(&hole)[12], 0.0);
    assert_eq!(scintillation_spatial_at(&hole, 12), 0.0);
    // A size/pixel mismatch reads all zero rather than panicking.
    hole.width = 6;
    assert!(scintillation_spatial(&hole).iter().all(|s| *s == 0.0));
}

#[test]
fn index_combines_sixty_forty() {
    let mut image = map_from(3, |_, _| true);
    image.scint_computed = true;
    // Pixel 4: two of five poses returned, so the Bernoulli variance is the cap 0.24.
    image.pixels[4].scint_returned = 2;
    image.pixels[4].scint_poses = 5;
    let spatial = vec![50.0; 9];
    let index = scintillation_index(&image, &spatial, 4).unwrap();
    assert!(
        (index - 0.4_f32.mul_add(100.0, 0.6 * 50.0)).abs() < 1e-3,
        "{index}"
    );
    // Never flipping: temporal zero, spatial only.
    image.pixels[4].scint_returned = 5;
    let steady = scintillation_index(&image, &spatial, 4).unwrap();
    assert!((steady - 30.0).abs() < 1e-3, "{steady}");
    // Not computed: no index.
    assert!(scintillation_index(&image, &spatial, 3).is_none());
    // The aggregate sits between its two terms and is finite.
    let aggregate = image.scintillation_pct();
    assert!((0.0..=100.0).contains(&aggregate));
    // The hover's single-pixel read equals the full pass bit for bit, flicker or not.
    let full = scintillation_spatial(&image);
    for i in 0..9 {
        assert_eq!(
            scintillation_index_at(&image, i).map(f32::to_bits),
            scintillation_index(&image, &full, i).map(f32::to_bits)
        );
    }
}

#[test]
fn pose_none_with_scintillation_is_invalid_input() {
    assert_eq!(
        evaluate(false, true, None, || false).unwrap_err(),
        OpticalImageError::InvalidInput
    );
    for bad in [
        [f32::NAN, 0.4, 2.4],
        [0.6, 0.4, 0.0],
        [0.6, f32::INFINITY, 2.4],
    ] {
        assert_eq!(
            evaluate(false, true, Some(bad), || false).unwrap_err(),
            OpticalImageError::InvalidInput
        );
    }
}

#[test]
fn cancellation_inside_the_offset_loop() {
    // Calls the plain evaluation makes: the threshold lies just past them, inside the
    // temporal loop of the scintillation run.
    let plain_calls = Cell::new(0_u32);
    evaluate(false, false, None, || {
        plain_calls.set(plain_calls.get() + 1);
        false
    })
    .unwrap();
    // A non-cancelling counter pins where the checks sit: one at the entry of `apply` and
    // one before each of the four non-base offsets, for every hit pixel (a hit pixel whose
    // base ray misses leaves after the entry check).
    let counted = Cell::new(0_u32);
    let image = evaluate(false, true, Some(POSE), || {
        counted.set(counted.get() + 1);
        false
    })
    .unwrap();
    let expected_extra: u32 = image
        .pixels
        .iter()
        .filter(|p| p.hits > 0)
        .map(|p| if p.scint_poses > 0 { 5 } else { 1 })
        .sum();
    assert!(expected_extra > 0, "the stone must produce hit pixels");
    assert_eq!(
        counted.get(),
        plain_calls.get() + expected_extra,
        "one check per pose, inside the offset loop"
    );
    let limit = plain_calls.get() + 3;
    let count = Cell::new(0_u32);
    let cancel_late = || {
        count.set(count.get() + 1);
        count.get() > limit
    };
    assert_eq!(
        evaluate(false, true, Some(POSE), cancel_late).unwrap_err(),
        OpticalImageError::Cancelled
    );
    count.set(0);
    assert!(evaluate(false, false, None, cancel_late).is_ok());
}

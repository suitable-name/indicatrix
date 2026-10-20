use super::*;
use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
};
#[test]
fn perspective_counts_conserve_hits_and_repeat() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let material = GemMaterial::diamond();
    let r = OpticalImageRequest {
        camera: &camera,
        width: 30,
        height: 19,
        samples: 4,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95),
        want_fire: true,
        want_scintillation: false,
        pose: None,
    };
    let a = evaluate_optical_image(&r, || false).unwrap();
    let b = evaluate_optical_image(&r, || false).unwrap();
    for (a, b) in a.pixels.iter().zip(&b.pixels) {
        assert_eq!(a.classes.iter().sum::<u32>(), a.hits);
        assert_eq!(a.classes, b.classes);
        assert_eq!(a.fire_sum_deg.to_bits(), b.fire_sum_deg.to_bits());
        assert!(a.hits <= r.samples);
    }
    assert!(a.pixels.iter().any(|p| p.hits == 0));
    assert!(a.total.hits > 0);
}
#[test]
fn cancellation_and_size_refusal_are_explicit() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 1.0, 2.4, 42.0);
    let mut r = OpticalImageRequest {
        camera: &camera,
        width: 10,
        height: 10,
        samples: 1,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.0, 0.0),
        want_fire: false,
        want_scintillation: false,
        pose: None,
    };
    assert!(matches!(
        evaluate_optical_image(&r, || true),
        Err(OpticalImageError::Cancelled)
    ));
    r.width = u32::MAX;
    assert!(matches!(
        evaluate_optical_image(&r, || false),
        Err(OpticalImageError::ResourceLimit)
    ));
}

#[test]
fn zero_dispersion_has_no_false_fire_and_optional_fire_preserves_base_counts() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let mut material = GemMaterial::diamond();
    material.dispersion = crate::optics::dispersion::DispersionModel::Cauchy {
        a: 2.42,
        b: 0.0,
        c: 0.0,
    };
    let camera = Camera::new(0.6, 1.2, 2.4, 42.0);
    let mut r = OpticalImageRequest {
        camera: &camera,
        width: 40,
        height: 31,
        samples: 4,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.0, 0.0),
        want_fire: true,
        want_scintillation: false,
        pose: None,
    };
    let fire = evaluate_optical_image(&r, || false).unwrap();
    assert!(fire.total.classes[2] > 0);
    assert!(fire.pixels.iter().all(|p| p.fire_sum_deg == 0.0));
    r.want_fire = false;
    let base = evaluate_optical_image(&r, || false).unwrap();
    assert_eq!(base.total.classes, fire.total.classes);
    assert!(!base.fire_computed);
}
#[test]
fn row_tiles_match_full_projection_and_pattern_bytes_including_alpha() {
    use crate::renderer::optical_overlay::{OpticalPresentation, optical_rgba};
    let planes = StandardGemCuts::emerald_cut();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.6, 0.8, 2.4, 42.0);
    let mut r = OpticalImageRequest {
        camera: &camera,
        width: 37,
        height: 29,
        samples: 4,
        projection_size: Some([37, 29]),
        tile_origin: Some([0, 0]),
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.0, 0.0),
        want_fire: true,
        want_scintillation: false,
        pose: None,
    };
    let full = evaluate_optical_image(&r, || false).unwrap();
    let style = OpticalPresentation {
        layers: [true; 4],
        opacity: 0.45,
        map_mode: false,
        high_contrast: false,
        scintillation: false,
    };
    let expected = optical_rgba(&full, style);
    let mut tiled = Vec::new();
    for y in (0..29).step_by(8) {
        r.height = 8.min(29 - y);
        r.tile_origin = Some([0, y]);
        let tile = evaluate_optical_image(&r, || false).unwrap();
        tiled.extend(optical_rgba(&tile, style));
    }
    assert_eq!(tiled, expected);
    r.tile_origin = Some([37, 0]);
    assert!(matches!(
        evaluate_optical_image(&r, || false),
        Err(OpticalImageError::InvalidInput)
    ));
}
#[test]
fn invalid_planes_and_camera_basis_are_refused_and_concave_tools_participate() {
    use crate::geometry::tool::ToolPrimitive;
    use glam::Vec3;
    let mut planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let mut camera = Camera::new(0.6, 1.2, 2.4, 42.0);
    let tools = [ToolPrimitive::cylinder(
        Vec3::new(0.0, 0.44, 0.0),
        Vec3::X,
        0.3,
        2.0,
    )];
    {
        let mut r = OpticalImageRequest {
            camera: &camera,
            width: 32,
            height: 32,
            samples: 4,
            projection_size: None,
            tile_origin: None,
            geometry: StoneGeometry::planes_only(&planes),
            material: &material,
            environment: LightingPreset::IsoHemisphere.studio(1.0, 0.0, 0.0),
            want_fire: false,
            want_scintillation: false,
            pose: None,
        };
        let planar = evaluate_optical_image(&r, || false).unwrap();
        r.geometry.tools = &tools;
        let concave = evaluate_optical_image(&r, || false).unwrap();
        assert!(
            planar.pixels.iter().zip(concave.pixels.iter()).any(
                |(a, b)| a.classes != b.classes || a.centre.entry_facet != b.centre.entry_facet
            )
        );
    }
    planes[0].d = f32::NAN;
    let r = OpticalImageRequest {
        camera: &camera,
        width: 3,
        height: 3,
        samples: 1,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.0, 0.0),
        want_fire: false,
        want_scintillation: false,
        pose: None,
    };
    assert!(matches!(
        evaluate_optical_image(&r, || false),
        Err(OpticalImageError::InvalidInput)
    ));
    planes[0].d = -1.0;
    camera.right = camera.forward;
    let r = OpticalImageRequest {
        camera: &camera,
        width: 3,
        height: 3,
        samples: 1,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.0, 0.0),
        want_fire: false,
        want_scintillation: false,
        pose: None,
    };
    assert!(matches!(
        evaluate_optical_image(&r, || false),
        Err(OpticalImageError::InvalidInput)
    ));
}

/// The overlay's pixel `x` is the beauty pixel `x`: the renderer (and its un-jittered guide
/// pass) centres pixel `x` at screen coordinate `x`, so the overlay's centre sample must
/// hit exactly the facet the guide ray of the same pixel hits, or every class edge lands
/// half a pixel off the picture.
#[test]
fn centre_samples_hit_the_same_facet_as_the_beauty_guide_pass() {
    use crate::optics::raytracer::{build_plane_soa, intersect_stone::intersect_stone_soa};
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let (width, height) = (48u32, 36u32);
    let r = OpticalImageRequest {
        camera: &camera,
        width,
        height,
        samples: 1,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment: LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95),
        want_fire: false,
        want_scintillation: false,
        pose: None,
    };
    let image = evaluate_optical_image(&r, || false).unwrap();
    let soa = build_plane_soa(&planes);
    let mut covered = 0;
    for y in 0..height {
        for x in 0..width {
            let ray =
                camera.generate_ray(x as f32, y as f32, width as f32, height as f32, 0.0, 0.0);
            let guide = intersect_stone_soa(ray, &soa, soa.len(), &[]).map(|h| h.facet_idx);
            let pixel = &image.pixels[(y * width + x) as usize];
            assert_eq!(pixel.centre.entry_facet, guide, "pixel {x},{y}");
            covered += usize::from(guide.is_some());
        }
    }
    assert!(
        covered > 100,
        "the stone must cover part of the frame: {covered}"
    );
}

/// The beauty render centres the head-shadow cone on each pixel's own eye direction. Away
/// from the image centre that differs from the camera axis by more than the cone, so a
/// classifier fixed to the axis scores rim pixels the wrong way round.
#[test]
fn perspective_rays_use_their_own_eye_direction_for_the_observer_cone() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 1.2, 2.4, 42.0);
    let environment = LightingPreset::LightTent
        .studio(1.0, 0.85, 0.95)
        .with_head_shadow(30.0);
    let (width, height) = (41u32, 41u32);
    let r = OpticalImageRequest {
        camera: &camera,
        width,
        height,
        samples: 1,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry::planes_only(&planes),
        material: &material,
        environment,
        want_fire: false,
        want_scintillation: false,
        pose: None,
    };
    let image = evaluate_optical_image(&r, || false).unwrap();
    let prepared =
        PreparedEval::new(StoneGeometry::planes_only(&planes), &material, environment).unwrap();
    let ctx = ApertureSampleContext {
        stone: prepared.stone(),
        nd: prepared.nd,
        n_f: prepared.n_f,
        n_c: prepared.n_c,
        cam_forward: camera.forward,
        cam_right: camera.right,
        cam_up: camera.up,
        fan: prepared.fan,
        lighting: &prepared.lighting,
        want_fire: false,
    };
    let bucket = |class: &Option<RayClassification>| match class {
        Some(RayClassification::Extinct) => 1,
        Some(RayClassification::Returned(_)) => 2,
        _ => 0,
    };
    let mut moved = 0;
    for y in 0..height {
        for x in 0..width {
            let ray =
                camera.generate_ray(x as f32, y as f32, width as f32, height as f32, 0.0, 0.0);
            let own = classify_ray(&ctx, ray, -ray.dir, None);
            let axis = classify_ray(&ctx, ray, -camera.forward, None);
            let pixel = &image.pixels[(y * width + x) as usize];
            // The image path is the per-pixel eye direction, bucket for bucket.
            assert_eq!(
                pixel.classes[1],
                u32::from(bucket(&own) == 1),
                "pixel {x},{y}"
            );
            assert_eq!(
                pixel.classes[2],
                u32::from(bucket(&own) == 2),
                "pixel {x},{y}"
            );
            moved +=
                usize::from(bucket(&own) != bucket(&axis) && bucket(&own) * bucket(&axis) != 0);
        }
    }
    assert!(
        moved > 0,
        "no pixel changed between extinction and brilliance when the cone followed the eye"
    );
}

/// Prints the per-pixel Fire distribution `FIRE_RANGE_DEG` is chosen from (run with
/// `--nocapture`): the 50th and 95th percentile of `fire_deg()` over the hit pixels, for a
/// high- and a low-dispersion stone at one pose. The owner picks the range, or a
/// per-material scale, from these numbers.
#[test]
fn fire_distribution_for_the_range_choice() {
    use crate::renderer::optical_overlay::FIRE_RANGE_DEG;
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.0, 1.2, 2.4, 42.0);
    let quartz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Quartz")
        .expect("the built-in table has Quartz");
    for material in [GemMaterial::diamond(), quartz] {
        let r = OpticalImageRequest {
            camera: &camera,
            width: 48,
            height: 48,
            samples: 4,
            projection_size: None,
            tile_origin: None,
            geometry: StoneGeometry::planes_only(&planes),
            material: &material,
            environment: LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95),
            want_fire: true,
            want_scintillation: false,
            pose: None,
        };
        let image = evaluate_optical_image(&r, || false).unwrap();
        let mut fire: Vec<f32> = image
            .pixels
            .iter()
            .filter(|p| p.hits > 0)
            .map(OpticalPixel::fire_deg)
            .collect();
        assert!(!fire.is_empty() && fire.iter().all(|f| f.is_finite() && *f >= 0.0));
        fire.sort_by(f32::total_cmp);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "q is in [0, 1], so the rounded index is in [0, fire.len() - 1]"
        )]
        let percentile = |q: f32| fire[((fire.len() - 1) as f32 * q).round() as usize];
        let saturated = fire.iter().filter(|f| **f >= FIRE_RANGE_DEG).count();
        println!(
            "{}: {} hit pixels, fire_deg p50 {:.4}, p95 {:.4}, max {:.4}; {} at or above FIRE_RANGE_DEG {}",
            material.name,
            fire.len(),
            percentile(0.5),
            percentile(0.95),
            fire[fire.len() - 1],
            saturated,
            FIRE_RANGE_DEG
        );
    }
}

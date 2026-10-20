//! The attribution recorder: it changes no bit of the traced radiance, and the events it
//! records label paths the way the analytical map's predicates do.

use super::super::{
    super::{
        camera::Ray, environment::LightingPreset, intersect::build_plane_soa, sampling::hash_u32,
    },
    PathEvents, PathTermination, trace_spectral_ray_attributed_soa_geom,
    trace_spectral_ray_with_finish_soa_geom,
};
use crate::{
    color::metrics::{
        AttributionLighting, ExtinctionReason, OpticalImageRequest, SpectralClass, classify_path,
        evaluate_optical_image,
    },
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{fluorescence::Fluorescence, materials::GemMaterial, raytracer::Camera},
};
use glam::Vec3;

/// A uniform `[-0.5, 0.5)` draw for sample `i` and `salt`.
fn centred(i: u32, salt: u32) -> f32 {
    (hash_u32(i ^ salt) as f32) / 4_294_967_295.0 - 0.5
}

/// An oblique camera ray from above that mostly meets the stone, varied per `i`.
fn ray_for(i: u32) -> Ray {
    Ray {
        origin: Vec3::new(centred(i, 0x1111) * 1.4, 2.5, centred(i, 0x2222) * 1.4),
        dir: Vec3::new(centred(i, 0x3333) * 0.8, -1.0, centred(i, 0x4444) * 0.8).normalize(),
    }
}

/// The seed and hero draw of sample `i`, the way the existing transport tests make them.
fn seed_and_hero(i: u32) -> (u32, f32) {
    let seed = 9000 + i;
    (seed, (hash_u32(seed) as f32) / 4_294_967_295.0)
}

/// Traces sample `i` of `ray` plainly and attributed, returning both results.
fn both(
    i: u32,
    ray: Ray,
    material: &GemMaterial,
    environment: crate::optics::raytracer::EnvironmentSource<'_>,
    max_bounces: u32,
) -> (Vec3, Vec3, PathEvents) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let geom = StoneGeometry::planes_only(&planes);
    let fluorescence = Fluorescence::none();
    let (seed, hero) = seed_and_hero(i);
    let plain = trace_spectral_ray_with_finish_soa_geom(
        ray,
        geom,
        &plane_soa,
        &[],
        material,
        fluorescence,
        max_bounces,
        environment,
        seed,
        hero,
        None,
    );
    let (attributed, events) = trace_spectral_ray_attributed_soa_geom(
        ray,
        geom,
        &plane_soa,
        &[],
        material,
        fluorescence,
        max_bounces,
        environment,
        seed,
        hero,
        None,
    );
    (plain, attributed, events)
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// The recorder only observes: 10 000 rays over three materials give the same bits with and
/// without it.
#[test]
fn attributed_radiance_equals_plain_radiance() {
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    for name in ["Diamond", "Ruby", "Zircon"] {
        let material = GemMaterial::by_name(name).expect("built-in material");
        for i in 0..3_334 {
            let (plain, attributed, _) = both(i, ray_for(i), &material, environment, 12);
            assert_eq!(bits(plain), bits(attributed), "{name} sample {i}");
        }
    }
}

/// A convex stone never lets a path back in after it left, an entered path always ends for a
/// reason the loop recorded, and a path that entered almost never loses itself inside.
///
/// "Almost": a ray inside a convex polyhedron can fail to find an exit only through
/// floating-point effects at a facet edge. The bounce loop restarts every ray `1e-4` past the
/// facet it hit, and `intersect_polyhedron_soa` reports a miss when the exit lies within
/// `1e-4` (`t_far <= 1e-4`) or when the restart point already sits outside a neighbouring
/// plane (`t_near > t_far`). Both need the hit to fall within about `1e-4` of an edge. Sample
/// 2015 of this fixture is such a case (measured 2026-10-20; the path's geometry does not
/// depend on the lighting, and this assertion was never calibrated). The tracer is left
/// alone: the lost sample is one of ~1e-3 and carries no systematic direction.
///
/// Bound, derived: the share of a facet surface within `delta = 1e-4` of an edge is
/// `(total edge length * 2 * delta) / surface area`. For the round brilliant (about 100
/// edges of mean length 0.4, surface area about 6 in the stone's units) that is
/// `40 * 2e-4 / 6 = 1.3e-3` per facet event. Counting every near-edge event as a loss (an
/// over-estimate, since many restarts still find a plane) and at most 12 events per path
/// (`max_bounces`), a path is lost with probability at most `12 * 1.3e-3 = 1.6e-2`. The
/// assertion allows 2 % of the entered paths; the one case seen in 2016 samples is ~0.1 %.
#[test]
fn a_convex_stone_never_reenters_and_rarely_loses_a_path_after_entry() {
    let environment = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);
    let lighting = AttributionLighting::new(environment);
    let material = GemMaterial::by_name("Diamond").expect("built-in material");
    let (mut entered, mut lost) = (0u32, 0u32);
    for i in 0..4_000 {
        let ray = ray_for(i);
        let (_, _, events) = both(i, ray, &material, environment, 12);
        assert!(!events.reentered, "sample {i}");
        let class = classify_path(&events, -ray.dir, &lighting);
        lost += u32::from(class == Some(SpectralClass::Extinct(ExtinctionReason::NoIntersection)));
        if events.first_facet.is_some_and(|(_, entered)| entered) {
            entered += 1;
            assert!(events.termination.is_some(), "sample {i}");
        }
    }
    assert!(
        entered > 200,
        "the fixture rays must often enter: {entered}"
    );
    // 2 % of the entered paths, see the derivation above (integer form avoids a float cast).
    assert!(
        lost * 50 <= entered,
        "{lost} of {entered} entered paths found no intersection inside a convex stone"
    );
}

/// A camera ray that never meets the stone records only its escape.
#[test]
fn a_missing_ray_records_only_the_escape() {
    let environment = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);
    let material = GemMaterial::by_name("Diamond").expect("built-in material");
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::Y,
    };
    let (_, _, events) = both(1, ray, &material, environment, 12);
    assert!(events.first_facet.is_none() && events.first_exit.is_none());
    assert_eq!(events.escaped_dir.map(bits), Some(bits(Vec3::Y)));
    assert_eq!(events.termination, Some(PathTermination::Escaped));
    let lighting = AttributionLighting::new(environment);
    assert_eq!(classify_path(&events, -ray.dir, &lighting), None);
}

/// Surface glare scales only the outside reflection: with the control at zero the glare
/// class delivers no luminance, and no sample changes class.
#[test]
fn glare_is_bounce_zero_outside() {
    let material = GemMaterial::by_name("Diamond").expect("built-in material");
    let run = |glare: f32| {
        // Uniform radiance: the table reflection delivers light whatever direction it takes.
        let environment = LightingPreset::IsoHemisphere
            .studio(1.0, 0.85, 0.95)
            .with_surface_glare(glare);
        let lighting = AttributionLighting::new(environment);
        let ray = Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        };
        let (mut glare_y, mut glare_count) = (0.0f32, 0);
        let mut classes = Vec::new();
        for i in 0..512 {
            let (_, xyz, events) = both(i, ray, &material, environment, 12);
            let class = classify_path(&events, -ray.dir, &lighting);
            if class == Some(SpectralClass::SurfaceGlare) {
                glare_y += xyz.y;
                glare_count += 1;
            }
            classes.push(class);
        }
        (glare_y, glare_count, classes)
    };
    let (full_y, full_count, full_classes) = run(1.0);
    let (none_y, none_count, none_classes) = run(0.0);
    assert!(full_count > 0, "the table reflection must occur");
    assert!(full_y > 0.0, "and deliver light under uniform lighting");
    assert!(none_y <= 1e-6, "glare 0.0 removes it: {none_y}");
    assert_eq!(full_count, none_count);
    assert_eq!(full_classes, none_classes);
}

/// A scattering, absorbing ruby at a 64-bounce budget ends paths by roulette and by
/// scattering absorption; a path that never exited is labelled with that reason.
#[test]
fn russian_roulette_and_scatter_are_reasons() {
    let environment = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);
    let lighting = AttributionLighting::new(environment);
    let mut material = GemMaterial::by_name("Ruby").expect("built-in material");
    material.scattering_sigma_s = 3.0;
    material.scattering_g = 0.5;
    let (mut roulette, mut absorbed, mut scattered) = (0, 0, 0);
    for i in 0..3_000 {
        let ray = ray_for(i);
        let (_, _, events) = both(i, ray, &material, environment, 64);
        scattered += u32::from(events.scattered);
        let class = classify_path(&events, -ray.dir, &lighting);
        match events.termination {
            Some(PathTermination::RussianRoulette) => roulette += 1,
            Some(PathTermination::ScatterAbsorbed) => absorbed += 1,
            _ => {}
        }
        if events.first_exit.is_none() && events.first_facet.is_some_and(|(_, entered)| entered) {
            let expected = match events.termination {
                Some(PathTermination::RussianRoulette) => ExtinctionReason::RussianRoulette,
                Some(PathTermination::ScatterAbsorbed) => ExtinctionReason::ScatterAbsorbed,
                Some(PathTermination::HitCap) => ExtinctionReason::BounceBudget,
                _ => ExtinctionReason::NoIntersection,
            };
            assert_eq!(class, Some(SpectralClass::Extinct(expected)), "sample {i}");
        }
    }
    assert!(scattered > 0, "a scattering material must scatter");
    assert!(absorbed > 0, "scatter absorption must occur");
    assert!(roulette > 0, "roulette must occur at a 64-bounce budget");
}

/// How often the spectral hero path's windowed label agrees with the analytical map's
/// d-line verdict on the identical camera rays (isotropic diamond, polished). The stochastic
/// Fresnel branch and the hero wavelength explain the disagreement; the plan's target is
/// 99 %, which no run has yet confirmed, so this prints the rate instead of asserting it.
#[test]
#[ignore = "pinned at Gate A: prints the measured agreement rate"]
fn windowed_matches_the_analytical_predicate_on_identical_rays() {
    const SIZE: u32 = 48;
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let material = GemMaterial::diamond();
    let environment = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);
    let request = OpticalImageRequest {
        camera: &camera,
        width: SIZE,
        height: SIZE,
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
    let image = evaluate_optical_image(&request, || false).expect("a valid request");
    let lighting = AttributionLighting::new(environment);
    let (mut compared, mut agreed) = (0u32, 0u32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = &image.pixels[(y * SIZE + x) as usize];
            if pixel.hits != 1 {
                continue;
            }
            let ray = camera.generate_ray(x as f32, y as f32, SIZE as f32, SIZE as f32, 0.0, 0.0);
            let analytical_windowed = pixel.classes[0] == 1;
            for i in 0..4 {
                let (_, _, events) = both((y * SIZE + x) * 4 + i, ray, &material, environment, 12);
                let spectral = classify_path(&events, -ray.dir, &lighting);
                compared += 1;
                agreed +=
                    u32::from((spectral == Some(SpectralClass::Windowed)) == analytical_windowed);
            }
        }
    }
    assert!(compared > 0);
    println!(
        "windowed agreement: {agreed} of {compared} ({:.2} %)",
        100.0 * f64::from(agreed) / f64::from(compared)
    );
}

//! The lit backdrop tray at tracer level: a camera ray that misses the stone, a ray that
//! leaks out of the stone's underside and a window all read the same environment function,
//! so under a tray the surround and the windows show the same radiance.
//!
//! The lookup-level equalities (`optics::raytracer::environment::tests_tray`) prove that the
//! function is the same at every bounce; these tests prove that the tracer reaches it with
//! weight 1 at the primary miss and with the Fresnel weights through a window.
//!
//! Method. The tracer returns XYZ built from stochastically sampled hero wavelengths, so the
//! expectations are never recomputed from the spectrum. Every comparison traces the quantities
//! to be compared with the SAME `(seed, hero_rand)` per sample, so the wavelengths agree sample
//! by sample and only linear, wavelength-independent factors remain. The observer head shadow is
//! switched off (`with_head_shadow(0.0)`): the tracer's observer is the reverse of the camera
//! ray, which would otherwise darken the lit models' environment around the viewing direction
//! and make a ray that looks down and a ray that looks up see different observers.

use glam::Vec3;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, LightingPreset, Ray, trace_spectral_ray},
    },
    render_setup::Backdrop,
};

/// The light pose every test here uses (radians), the grading default.
const YAW: f32 = 0.84;
const PITCH: f32 = 1.2;
/// Bounce cap: deep enough that a thin plate's internal reflections are not truncated
/// (the n = 1.5 plate loses (0.04)^k of its energy per extra pass, below 1e-9 by k = 6).
const MAX_BOUNCES: u32 = 32;
/// Mean refractive index of the plate (zero dispersion: the same at every wavelength).
const PLATE_INDEX: f32 = 1.5;
/// Samples per plate estimate.
const PLATE_SAMPLES: u32 = 4096;

/// `(seed, hero_rand)` of sample `i`: the hero draw in `[0, 1)` from 24 hashed bits.
fn draws(i: u32) -> (u32, f32) {
    let seed = indicatrix::optics::raytracer::hash_u32(i ^ 0x7A57_B0A7);
    let hero = (indicatrix::optics::raytracer::hash_u32(seed) >> 8) as f32 / 16_777_216.0;
    (seed, hero)
}

/// `preset` at exposure `exposure`, the test light pose, a tray radiance of `tray` and no head
/// shadow.
const fn environment(
    preset: LightingPreset,
    exposure: f32,
    tray: f32,
) -> EnvironmentSource<'static> {
    preset
        .studio(exposure, YAW, PITCH)
        .with_backdrop(tray)
        .with_head_shadow(0.0)
}

/// Luminance `Y` of sample `i` of `ray`.
fn trace_y(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    ray: Ray,
    environment: EnvironmentSource<'_>,
    i: u32,
) -> f32 {
    let (seed, hero) = draws(i);
    trace_spectral_ray(
        ray,
        planes,
        material,
        MAX_BOUNCES,
        environment,
        seed,
        hero,
        None,
    )
    .y
}

/// A ray looking straight down from above at horizontal offset `x`.
fn looking_down(x: f32) -> Ray {
    Ray {
        origin: Vec3::new(x, 2.5, 0.0),
        dir: -Vec3::Y,
    }
}

/// A ray looking straight up from below at horizontal offset `x`.
const fn looking_up(x: f32) -> Ray {
    Ray {
        origin: Vec3::new(x, -2.5, 0.0),
        dir: Vec3::Y,
    }
}

/// A thin colourless plate: a 1.6 x 1.6 x 0.1 box (two large facets, four sides).
fn plate() -> Vec<GpuFacetPlane> {
    const HALF_WIDTH: f32 = 0.8;
    const HALF_THICKNESS: f32 = 0.05;
    vec![
        GpuFacetPlane::new(Vec3::Y, -HALF_THICKNESS),
        GpuFacetPlane::new(-Vec3::Y, -HALF_THICKNESS),
        GpuFacetPlane::new(Vec3::X, -HALF_WIDTH),
        GpuFacetPlane::new(-Vec3::X, -HALF_WIDTH),
        GpuFacetPlane::new(Vec3::Z, -HALF_WIDTH),
        GpuFacetPlane::new(-Vec3::Z, -HALF_WIDTH),
    ]
}

/// The colourless, non-dispersive, isotropic plate material.
fn clear_glass() -> GemMaterial {
    GemMaterial::new_custom("tray test glass", PLATE_INDEX, 0.0, 0.0, [0.0, 0.0, 0.0])
}

/// Reflectance and transmittance of an incoherent lossless slab at normal incidence,
/// `(2 r / (1 + r), (1 - r) / (1 + r))` with `r = ((n - 1) / (n + 1))^2` per surface. They sum
/// to 1, and every internal path stays vertical, so a vertical ray sees only the directions
/// straight up (reflected) and straight down (transmitted).
fn slab_reflectance_transmittance() -> (f32, f32) {
    let r = ((PLATE_INDEX - 1.0) / (PLATE_INDEX + 1.0)).powi(2);
    (2.0 * r / (1.0 + r), (1.0 - r) / (1.0 + r))
}

fn assert_close(got: f32, expected: f32, relative: f32, what: &str) {
    assert!(
        (got - expected).abs() <= expected.abs() * relative,
        "{what}: {got} vs {expected} (relative tolerance {relative})"
    );
}

/// A camera ray that misses the stone takes the environment lookup of its own direction with
/// Stokes intensity 1 and MIS weight 1 (§11.1). Seen through the tracer:
///
/// - Straight down every non-UV preset returns `tray * exposure * S(lambda)`: the horizon blend
///   is exactly 0 and the visibility 1, so the result is linear in the tray and in the
///   exposure (1e-4 relative; the lookup level pins the same value to 1e-5).
/// - Under `IsoHemisphere` (`E/pi = 1` above the horizon) the ray looking down returns exactly
///   `tray` times the ray looking up, wavelength for wavelength.
#[test]
fn a_primary_miss_equals_the_environment_in_that_direction() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("built-in material");
    // 3.0 is far outside the stone on every side.
    let down = looking_down(3.0);
    for preset in LightingPreset::ALL {
        if preset.is_uv_lamp() {
            // A UV line is exactly zero in the visible: the tracer's Y is 0 there.
            continue;
        }
        for i in 0..8 {
            let at = |exposure: f32, tray: f32| {
                trace_y(
                    &planes,
                    &material,
                    down,
                    environment(preset, exposure, tray),
                    i,
                )
            };
            let reference = at(1.0, 1.0);
            assert!(
                reference > 0.0,
                "{preset:?}: the tray must be seen by a primary miss"
            );
            assert_close(
                at(1.0, 0.5),
                0.5 * reference,
                1e-4,
                &format!("{preset:?} tray"),
            );
            assert_close(
                at(2.0, 0.5),
                2.0 * at(1.0, 0.5),
                1e-4,
                &format!("{preset:?} exposure"),
            );
        }
    }

    let up = looking_up(3.0);
    for tray in [0.25f32, 0.9] {
        for i in 0..8 {
            let env = environment(LightingPreset::IsoHemisphere, 1.0, tray);
            let below = trace_y(&planes, &material, down, env, i);
            let above = trace_y(&planes, &material, up, env, i);
            assert_close(below, tray * above, 1e-4, "iso down = tray * up");
        }
    }
}

/// What one vertical ray sees through the plate, as a fraction of the two directions it can
/// reach. Returns `(mean through the plate, mean up, mean down)` over `PLATE_SAMPLES` samples
/// of luminance, the up and down means being the misses beside the plate with the same draws.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the means are compared at f32 precision against the f32 tracer output"
)]
fn plate_means(preset: LightingPreset, tray: f32) -> (f32, f32, f32) {
    let planes = plate();
    let material = clear_glass();
    let env = environment(preset, 1.0, tray);
    let (mut through, mut up, mut down) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..PLATE_SAMPLES {
        through += f64::from(trace_y(&planes, &material, looking_down(0.0), env, i));
        up += f64::from(trace_y(&planes, &material, looking_up(2.0), env, i));
        down += f64::from(trace_y(&planes, &material, looking_down(2.0), env, i));
    }
    let n = f64::from(PLATE_SAMPLES);
    ((through / n) as f32, (up / n) as f32, (down / n) as f32)
}

/// A thin parallel plate face-up (§11.1): the mean radiance through it is
/// `R_slab * up + T_slab * down`, where `down` is exactly what a ray beside the plate sees,
/// the tray, and `up` is the sky a reflection shows. (The plan's `tray * T^2` ignores the
/// reflected sky and the internal bounces; the slab formula is exact for this geometry.) The
/// same under the grading dome, the tent, the daylight dome and the white tray.
///
/// Tolerance: each sample returns `up_i` or `down_i` (times small weights), a Bernoulli draw of
/// probability `T_slab`, so the standard error of the mean is at most
/// `sqrt(T (1 - T) / 4096) * |up - down| = 0.0042 |up - down|`; 3 % of the larger of the two
/// means is at least seven standard errors.
#[test]
fn what_you_see_around_the_stone_equals_what_a_leaked_ray_sees() {
    let (reflectance, transmittance) = slab_reflectance_transmittance();
    for preset in [
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::DaylightDome,
        LightingPreset::WhiteTray,
    ] {
        let tray = Backdrop::White.resolve(preset, YAW, PITCH);
        assert!(tray > 0.0, "{preset:?}: a White tray has a radiance");
        let (through, up, down) = plate_means(preset, tray);
        assert!(down > 0.0 && up > 0.0, "{preset:?}: up {up}, down {down}");
        let expected = reflectance.mul_add(up, transmittance * down);
        let tolerance = 0.03 * up.max(down);
        assert!(
            (through - expected).abs() <= tolerance,
            "{preset:?}: through the plate {through}, expected {expected} \
             (R {reflectance} * up {up} + T {transmittance} * down {down}), tolerance {tolerance}"
        );
    }
}

/// The acceptance guard for D9 (§11.5-4): a window shows the tray. A vertical ray through the
/// plate returns the tray radiance times the slab transmittance (the Fresnel transmission of
/// both surfaces with the internal bounces; the glass is colourless, so no body absorption)
/// when the backdrop is White, and nothing from below when it is As Lit.
///
/// - White: the mean is at least 0.5 times the tray radiance (the plan's bound; the true value
///   is `(R_slab * up / down + T_slab)`, about 1.0 under the grading dome).
/// - As Lit: the below-the-horizon lookup is the preset's own ground, 0 under `IsoHemisphere`, so
///   only the reflected sky `R_slab * up` (7.7 %) remains: less than 0.15 times the White
///   value (true ratio 0.085, standard error of the As Lit mean 5.4 %, so seven errors).
/// - The two runs use the same draws and the same paths (no NEE under `IsoHemisphere`), so the
///   difference of the means is the transmitted share alone, `T_slab * tray` within 3 % (the
///   standard error is 0.45 %); and every sample is at least as bright under the tray as
///   without it.
#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the means are compared at f32 precision against the f32 tracer output"
)]
fn a_window_shows_the_backdrop() {
    let preset = LightingPreset::IsoHemisphere;
    let tray = Backdrop::White.resolve(preset, YAW, PITCH);
    let (_, transmittance) = slab_reflectance_transmittance();
    let planes = plate();
    let material = clear_glass();
    let lit = environment(preset, 1.0, tray);
    let as_lit = environment(preset, 1.0, 0.0);
    let (mut white, mut black, mut surround) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..PLATE_SAMPLES {
        let with_tray = trace_y(&planes, &material, looking_down(0.0), lit, i);
        let without = trace_y(&planes, &material, looking_down(0.0), as_lit, i);
        assert!(
            1e-5f32.mul_add(1.0 + without, with_tray) >= without,
            "sample {i}: the tray darkened a window ({with_tray} < {without})"
        );
        white += f64::from(with_tray);
        black += f64::from(without);
        surround += f64::from(trace_y(&planes, &material, looking_down(2.0), lit, i));
    }
    let n = f64::from(PLATE_SAMPLES);
    let (white, black, surround) = (
        (white / n) as f32,
        (black / n) as f32,
        (surround / n) as f32,
    );
    assert!(surround > 0.0, "the tray beside the plate must be seen");
    assert!(
        white >= 0.5 * surround,
        "a White window shows {white}, the tray beside it {surround}"
    );
    assert!(
        black < 0.15 * white,
        "an As Lit window shows {black}, the White one {white}"
    );
    assert_close(
        white - black,
        transmittance * surround,
        0.03,
        "the transmitted share of a window",
    );
}

/// The same guard on a real stone: the emerald cut face-up (it leaks, see
/// `detail_reasons_match_fates`). Same draws, same paths: no sample is darker under the tray
/// than As Lit (the tray only adds radiance below the horizon), the White total is larger, and
/// at least one ray comes back from a leak with more than 5 % of the tray radiance its own
/// wavelengths see beside the stone.
#[test]
fn an_emerald_cut_leaks_the_tray_into_its_windows() {
    const GRID: u32 = 32;
    let preset = LightingPreset::IsoHemisphere;
    let tray = Backdrop::White.resolve(preset, YAW, PITCH);
    let planes = StandardGemCuts::emerald_cut();
    let material = GemMaterial::by_name("Diamond").expect("built-in material");
    let lit = environment(preset, 1.0, tray);
    let as_lit = environment(preset, 1.0, 0.0);
    let (mut white, mut black) = (0.0f64, 0.0f64);
    let mut windows = 0u32;
    for iz in 0..GRID {
        for ix in 0..GRID {
            let i = iz * GRID + ix;
            let fx = (ix as f32 + 0.5) / GRID as f32 - 0.5;
            let fz = (iz as f32 + 0.5) / GRID as f32 - 0.5;
            let ray = Ray {
                origin: Vec3::new(fx * 1.4, 2.5, fz * 1.4),
                dir: Vec3::new(0.05, -1.0, 0.03).normalize(),
            };
            let with_tray = trace_y(&planes, &material, ray, lit, i);
            let without = trace_y(&planes, &material, ray, as_lit, i);
            assert!(
                1e-5f32.mul_add(1.0 + without, with_tray) >= without,
                "ray {i}: the tray darkened the stone ({with_tray} < {without})"
            );
            let beside = trace_y(&planes, &material, looking_down(3.0), lit, i);
            if with_tray - without > 0.05 * beside {
                windows += 1;
            }
            white += f64::from(with_tray);
            black += f64::from(without);
        }
    }
    println!(
        "emerald cut: {windows} of {} rays show the tray",
        GRID * GRID
    );
    assert!(white > black, "White {white} vs As Lit {black}");
    assert!(windows >= 1, "no ray of the emerald cut shows the tray");
}

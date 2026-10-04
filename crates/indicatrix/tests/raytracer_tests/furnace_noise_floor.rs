//! Measures what the CPU white-furnace anchors' tolerances have to cover, so that they
//! can be set from measured runs instead of by hand.
//!
//! Each anchor in the default suite compares one furnace mean with the analytic radiance
//! at a tolerance of 5 to 8 percent. Two things sit inside that tolerance. The first is
//! the bounce cap: a path still inside the gem when the cap is reached is dropped with
//! its energy, so a furnace traced at cap 12 or 16 reads low by a systematic fraction
//! (about one to two percent here), while the same scene at [`HIGH_CAP`] reads the
//! environment's radiance within noise. The second is the sampling noise at the anchor's
//! budget. A branch mis-weighted by a few percent hides inside the sum of the two.
//!
//! This run repeats every anchor's scene over [`SEEDS`] independent seeds at the anchor's
//! own cap and sample count, at 4 and 16 times that count, and once at [`HIGH_CAP`]. Per
//! level it prints the bias (the mean signed relative error over the seeds), the noise
//! (their standard deviation) and the worst single run; then the truncation loss (the
//! bias at the anchor's cap less the bias at the high cap), the cross-cap drift the
//! existing drift check would see, and the tolerance the measurements justify at the
//! anchor's own settings (`|bias| + 4 sigma`). It fails on three findings only: a bias
//! at the high cap beyond four standard errors (a real energy error, not truncation),
//! noise that does not fall with the square root of the sample count (the extrapolation
//! would be invalid), or a justified tolerance above the one the anchor's test uses.
//!
//! The seed salts differ above the sample-index bits: the fixture combines the salt and
//! the sample index by exclusive-or, so salts that differed only in their low bits would
//! permute the samples' seeds instead of changing them, and every run would be the same.
//!
//! Ignored by default (minutes in release, hours unoptimised). Run it with
//!
//! ```text
//! cargo test -p indicatrix --release --test raytracer_tests -- furnace_noise_floor --ignored --nocapture
//! ```

use std::thread;

use glam::Vec3;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, FacetFinish, Ray, trace_spectral_ray, trace_spectral_ray_with_finish,
        },
    },
    renderer::env_map::EnvironmentMap,
};

use crate::fixtures::{bruted_girdle_finishes, furnace_mean_xyz, uniform_furnace_target};

/// Radiance of the uniform environment, as in every anchor.
const L0: f32 = 2.5;
/// Independent runs per level; each gets its own seed salt and its own thread. A standard
/// deviation estimated from eight runs is itself uncertain by about a quarter, which made
/// the noise-ratio check below trip by chance; sixteen halves that.
const SEEDS: usize = 16;
/// Multiples of the anchor's own samples per pixel that are measured at its own cap.
const LADDER: [u32; 3] = [1, 4, 16];
/// A bounce cap at which the truncation loss is negligible.
const HIGH_CAP: u32 = 256;
/// Standard deviations of headroom a justified tolerance leaves above the bias.
const SIGMA: f32 = 4.0;

/// One furnace anchor of the default suite: its scene and the test that pins it.
struct Anchor {
    /// Short name for the report.
    name: &'static str,
    /// The default-suite test whose tolerance this measures.
    test: &'static str,
    /// That test's tolerance on the relative error.
    tolerance: f32,
    /// That test's samples per pixel.
    spp: u32,
    /// That test's bounce cap.
    max_bounces: u32,
    /// The material.
    material: GemMaterial,
    /// Per-facet finishes, `None` for all polished.
    finishes: Option<Vec<FacetFinish>>,
}

/// Bias, noise and worst run of the relative error at one cap and sample count.
#[derive(Clone, Copy)]
struct Level {
    /// Bounce cap of every run at this level.
    max_bounces: u32,
    /// Samples per pixel of every run at this level.
    spp: u32,
    /// Mean signed relative error over the seeds, per XYZ component.
    bias: [f32; 3],
    /// Sample standard deviation of the signed relative error, per component.
    noise: [f32; 3],
    /// Largest absolute relative error of any single run, per component.
    worst: [f32; 3],
}

/// A colorless, non-dispersive custom material with birefringence `delta`.
fn probe(name: &'static str, delta: f32) -> GemMaterial {
    GemMaterial::new_custom(name, 1.5, 0.0, delta, [0.0, 0.0, 0.0])
}

/// The anchors, with the caps, sample counts and tolerances their tests use today.
fn anchors(plane_count: usize) -> Vec<Anchor> {
    let frosted = || Some(bruted_girdle_finishes(plane_count));
    vec![
        Anchor {
            name: "birefringent",
            test: "birefringent_white_furnace_energy_conservation_holds (cross-cap drift 0.15)",
            tolerance: 0.05,
            spp: 64,
            max_bounces: 12,
            material: probe("birefringent furnace probe", 0.03),
            finishes: None,
        },
        Anchor {
            name: "frosted girdle, birefringent",
            test: "frosted_girdle_birefringent_white_furnace_energy_conservation_holds",
            tolerance: 0.05,
            spp: 64,
            max_bounces: 12,
            material: probe("frosted birefringent furnace probe", 0.03),
            finishes: frosted(),
        },
        Anchor {
            name: "frosted girdle, isotropic",
            test: "frosted_girdle_white_furnace_energy_conservation_still_holds",
            tolerance: 0.06,
            spp: 96,
            max_bounces: 12,
            material: probe("frosted furnace probe", 0.0),
            finishes: frosted(),
        },
        Anchor {
            name: "edge rounding",
            test: "intersect::tests::edge_rounding_white_furnace_energy_conservation_holds",
            tolerance: 0.06,
            spp: 96,
            max_bounces: 16,
            material: probe("edge rounding furnace probe", 0.0).with_edge_rounding(0.03),
            finishes: None,
        },
        Anchor {
            name: "lossless scattering",
            test: "scattering::tests::lossless_scattering_white_furnace_energy_conservation_holds",
            tolerance: 0.08,
            spp: 96,
            max_bounces: 16,
            material: probe("scattering furnace probe", 0.0).with_scattering(1.2, 0.4),
            finishes: None,
        },
    ]
}

/// Mean XYZ of one furnace run of `anchor` at `max_bounces` with `spp` samples per pixel
/// and the seed `salt`.
fn furnace_mean(
    anchor: &Anchor,
    planes: &[GpuFacetPlane],
    env: &EnvironmentMap,
    max_bounces: u32,
    spp: u32,
    salt: u32,
) -> Vec3 {
    let trace = |ray: Ray, seed: u32, hero: f32| {
        anchor.finishes.as_deref().map_or_else(
            || {
                trace_spectral_ray(
                    ray,
                    planes,
                    &anchor.material,
                    max_bounces,
                    EnvironmentSource::HdrMap(env),
                    seed,
                    hero,
                    None,
                )
            },
            |finishes| {
                trace_spectral_ray_with_finish(
                    ray,
                    planes,
                    finishes,
                    &anchor.material,
                    max_bounces,
                    EnvironmentSource::HdrMap(env),
                    seed,
                    hero,
                    None,
                )
            },
        )
    };
    furnace_mean_xyz(spp, salt, trace).0
}

/// Signed relative error of each component of `mean` against `target`.
fn relative_error(mean: Vec3, target: Vec3) -> [f32; 3] {
    let rel = |v: f32, t: f32| (v - t) / t.abs().max(1e-6);
    [
        rel(mean.x, target.x),
        rel(mean.y, target.y),
        rel(mean.z, target.z),
    ]
}

/// Runs `anchor` over the seeds at `max_bounces` and `spp`, one thread per seed, and
/// summarises the relative errors. The array of handles is built eagerly, so every run
/// starts before the first is joined.
fn measure(
    anchor: &Anchor,
    planes: &[GpuFacetPlane],
    env: &EnvironmentMap,
    target: Vec3,
    max_bounces: u32,
    spp: u32,
) -> Level {
    let errors: [[f32; 3]; SEEDS] = thread::scope(|scope| {
        let runs: [_; SEEDS] = std::array::from_fn(|seed| {
            // Above the sample-index bits, see the module doc.
            let salt = 0x4E46_0000 ^ ((seed as u32 + 1) << 16);
            scope.spawn(move || {
                relative_error(
                    furnace_mean(anchor, planes, env, max_bounces, spp, salt),
                    target,
                )
            })
        });
        runs.map(|run| run.join().expect("a furnace run finishes"))
    });
    let n = SEEDS as f32;
    let mut level = Level {
        max_bounces,
        spp,
        bias: [0.0; 3],
        noise: [0.0; 3],
        worst: [0.0; 3],
    };
    for c in 0..3 {
        let mean = errors.iter().map(|e| e[c]).sum::<f32>() / n;
        let variance = errors
            .iter()
            .map(|e| e[c] - mean)
            .fold(0.0f32, |acc, d| d.mul_add(d, acc))
            / (n - 1.0);
        level.bias[c] = mean;
        level.noise[c] = variance.sqrt();
        level.worst[c] = errors.iter().map(|e| e[c].abs()).fold(0.0, f32::max);
    }
    level
}

/// The report line of one level.
fn describe(level: &Level) -> String {
    let [bx, by, bz] = level.bias;
    let [nx, ny, nz] = level.noise;
    let [wx, wy, wz] = level.worst;
    format!(
        "cap {:>3} spp {:>5}: bias ({bx:+.4}, {by:+.4}, {bz:+.4})  noise ({nx:.4}, {ny:.4}, \
         {nz:.4})  worst ({wx:.4}, {wy:.4}, {wz:.4})",
        level.max_bounces, level.spp
    )
}

/// `value` rounded up to the next 0.005.
fn round_up(value: f32) -> f32 {
    (value / 0.005).ceil() * 0.005
}

/// The tolerance the measurements at the anchor's own settings justify: the largest
/// `|bias| + SIGMA * noise` over the components, rounded up to the next 0.005.
fn justified_tolerance(own: &Level) -> f32 {
    round_up(
        (0..3)
            .map(|c| SIGMA.mul_add(own.noise[c], own.bias[c].abs()))
            .fold(0.0, f32::max),
    )
}

/// The cross-cap drift tolerance the measurements justify: the largest difference of
/// the two levels' biases plus `SIGMA` times the noise of that difference.
fn justified_drift(own: &Level, high: &Level) -> f32 {
    round_up(
        (0..3)
            .map(|c| {
                SIGMA.mul_add(
                    own.noise[c].hypot(high.noise[c]),
                    (own.bias[c] - high.bias[c]).abs(),
                )
            })
            .fold(0.0, f32::max),
    )
}

/// Records what the levels of `anchor` say against its test: an energy error at the
/// high cap beyond the noise, noise that does not fall with the square root of the
/// sample count, or a tolerance below what the measurements need.
fn check(anchor: &Anchor, levels: &[Level], high: &Level, findings: &mut Vec<String>) {
    let own = &levels[0];
    let deepest = levels.last().expect("a level");
    for c in 0..3 {
        let standard_error = high.noise[c] / (SEEDS as f32).sqrt();
        if high.bias[c].abs() > SIGMA * standard_error {
            findings.push(format!(
                "{}: component {c} bias {:+.4} at cap {HIGH_CAP} exceeds {SIGMA} standard errors \
                 ({standard_error:.4}): an energy error beyond the truncation loss",
                anchor.name, high.bias[c]
            ));
        }
        // Sixteen times the samples should divide the noise by four; the ratio of two
        // estimated standard deviations scatters by about a quarter of itself, so the
        // window is wide, and a broken law would still show as a ratio near one.
        let ratio = own.noise[c] / deepest.noise[c].max(1e-9);
        if !(1.5..=10.0).contains(&ratio) {
            findings.push(format!(
                "{}: component {c} noise ratio {ratio:.2} between {} and {} spp is not near the \
                 expected 4",
                anchor.name, own.spp, deepest.spp
            ));
        }
    }
    let justified = justified_tolerance(own);
    if justified > anchor.tolerance {
        findings.push(format!(
            "{}: the measurements need {justified:.3}, above the tolerance {:.3} its test uses",
            anchor.name, anchor.tolerance
        ));
    }
}

/// Prints the summary lines of one anchor: truncation loss, cross-cap drift and the
/// justified tolerance.
fn report(anchor: &Anchor, own: &Level, high: &Level) {
    let [tx, ty, tz] = [0, 1, 2].map(|c| own.bias[c] - high.bias[c]);
    println!(
        "    truncation loss at cap {}: ({tx:+.4}, {ty:+.4}, {tz:+.4})",
        anchor.max_bounces
    );
    println!(
        "    cross-cap drift to cap {HIGH_CAP}: the measurements justify {:.3} (the birefringent \
         anchor checks 0.150 today)",
        justified_drift(own, high)
    );
    println!(
        "    tolerance {:.3} today; the measurements justify {:.3}",
        anchor.tolerance,
        justified_tolerance(own)
    );
}

#[test]
#[ignore = "a measurement over millions of samples: run it in release with --ignored \
            --nocapture, see the module doc"]
fn furnace_noise_floor_and_the_tolerances_it_justifies() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let env = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);
    let target = uniform_furnace_target(L0);
    let mut findings = Vec::new();
    for anchor in anchors(planes.len()) {
        println!(
            "[{}] pins `{}` at tolerance {:.3} with cap {} and {} spp",
            anchor.name, anchor.test, anchor.tolerance, anchor.max_bounces, anchor.spp
        );
        let levels: Vec<Level> = LADDER
            .iter()
            .map(|&multiple| {
                measure(
                    &anchor,
                    &planes,
                    &env,
                    target,
                    anchor.max_bounces,
                    anchor.spp * multiple,
                )
            })
            .collect();
        let high = measure(&anchor, &planes, &env, target, HIGH_CAP, anchor.spp);
        for level in levels.iter().chain([&high]) {
            println!("    {}", describe(level));
        }
        report(&anchor, &levels[0], &high);
        check(&anchor, &levels, &high, &mut findings);
    }
    assert!(findings.is_empty(), "{}", findings.join("\n"));
}

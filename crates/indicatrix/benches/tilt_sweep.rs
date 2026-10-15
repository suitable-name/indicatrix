//! Tilt Performance sweep (BLD-2; findings PERF-1/PERF-2 in
//! `review_perf_2026-10-12_fable.md`, section 5).
//!
//! `evaluate_all_axes_profiles_geom` is 724 raytrace evaluations (4 axes x 181 tilts) on a
//! standard round brilliant. Expected before any fix, 16-core AVX-512 host, release: about
//! 1.4 s serial (`Some(1)`), the parallel entry a fraction of that scaled by the core count.
//! The cut app's cost probe (RBC-445 Full objective, 1.2-1.4 s) is dominated by this sweep.
//!
//! Run: `cargo bench -p indicatrix --bench tilt_sweep`.

use divan::black_box;
use indicatrix::{
    color::metrics::{evaluate_all_axes_profiles_geom, evaluate_all_axes_profiles_geom_threaded},
    geometry::{StandardGemCuts, tool::StoneGeometry},
    optics::{GemMaterial, raytracer::LightingPreset},
};

fn main() {
    divan::main();
}

/// The parallel sweep with the machine's parallelism (the cut app's call).
#[divan::bench(sample_count = 5, sample_size = 1)]
fn all_axes_parallel(bencher: divan::Bencher) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    bencher.bench_local(|| {
        black_box(evaluate_all_axes_profiles_geom(
            black_box(StoneGeometry::planes_only(&planes)),
            black_box(&material),
            black_box(env),
        ))
    });
}

/// The serial sweep on the calling thread (`Some(1)`), the per-core baseline.
#[divan::bench(sample_count = 3, sample_size = 1)]
fn all_axes_serial(bencher: divan::Bencher) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    bencher.bench_local(|| {
        black_box(evaluate_all_axes_profiles_geom_threaded(
            black_box(StoneGeometry::planes_only(&planes)),
            black_box(&material),
            black_box(env),
            Some(1),
        ))
    });
}

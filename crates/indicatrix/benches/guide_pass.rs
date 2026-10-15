//! Guide prepass (depth / normal / facet id / path signature) per camera pose (BLD-2;
//! `review_perf_2026-10-12_fable.md` section 5, gap "guide prepass with `n_d > 1`").
//!
//! `n_d = 0.0` is the plain first-hit capture; `n_d = 1.76` also fills the interior path
//! signature (the `K = 3` TIR walk), the expensive variant the live loop runs on a pose
//! change. Expected before any fix on a 16-core host, release: the plain pass a few ms at
//! 800x600 and ~10-20 ms at 1080p; the signature pass several times that (UI-1: the live
//! loop spends about 82 ms per iteration at 1080p on guide generation plus signature).
//! The pass uses static row chunks (`guide_pass.rs:405-475`), so thread imbalance shows here.
//!
//! Run: `cargo bench -p indicatrix --bench guide_pass`.

use divan::black_box;
use indicatrix::{
    geometry::{GpuFacetPlane, StandardGemCuts},
    optics::Camera,
    renderer::guide_pass::generate_guide_buffers_with_index,
};

fn main() {
    divan::main();
}

/// Pose the cut app's default view uses (see the unit tests of the pass).
fn camera() -> Camera {
    Camera::new(0.60, 0.45, 2.4, 42.0)
}

/// One guide pass at `width` x `height` with reference index `n_d`.
fn run(bencher: divan::Bencher, width: u32, height: u32, n_d: f32) {
    let planes: Vec<GpuFacetPlane> = StandardGemCuts::standard_round_brilliant();
    let camera = camera();
    bencher.bench_local(|| {
        black_box(generate_guide_buffers_with_index(
            black_box(width),
            black_box(height),
            black_box(&camera),
            black_box(&planes),
            black_box(n_d),
        ))
    });
}

/// 800x600, no signature.
#[divan::bench(sample_count = 20)]
fn w800_nd0(bencher: divan::Bencher) {
    run(bencher, 800, 600, 0.0);
}

/// 800x600, path signature (diamond-like index).
#[divan::bench(sample_count = 20)]
fn w800_nd176(bencher: divan::Bencher) {
    run(bencher, 800, 600, 1.76);
}

/// 1920x1080, no signature.
#[divan::bench(sample_count = 10)]
fn w1080p_nd0(bencher: divan::Bencher) {
    run(bencher, 1920, 1080, 0.0);
}

/// 1920x1080, path signature.
#[divan::bench(sample_count = 10)]
fn w1080p_nd176(bencher: divan::Bencher) {
    run(bencher, 1920, 1080, 1.76);
}

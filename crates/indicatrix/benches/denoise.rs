//! A-Trous denoiser per frame size (BLD-2; finding PERF-4 in
//! `review_perf_2026-10-12_fable.md`, section 5, table row 7).
//!
//! Expected before any fix, 16-core host, release: 267 ms per call at 800x600 and
//! 1 072 ms at 1080p (the logged denoiser line); after PERF-4 about 70-130 ms and
//! 270-500 ms. The guides come from the real guide pass on a round brilliant, the colour
//! is deterministic synthetic noise at 16 spp (taper well above the identity cut-off), so
//! the edge-stopping terms do real work. The "sig" variants add the path signature
//! (`n_d = 1.76`); a persistent [`AtrousDenoiser`] is reused across iterations like the
//! live loop does.
//!
//! Run: `cargo bench -p indicatrix --bench denoise`.

use divan::black_box;
use glam::Vec3;
use indicatrix::{
    geometry::StandardGemCuts,
    optics::Camera,
    renderer::{
        denoise::{AtrousDenoiser, AtrousParams, GBuffers},
        guide_pass::generate_guide_buffers_with_index,
    },
};

fn main() {
    divan::main();
}

/// Deterministic noisy colour: xorshift32 per pixel, values in `0.0..2.0` per channel.
fn noisy_color(len: usize) -> Vec<Vec3> {
    let mut state = 0x9e37_79b9_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        f32::from(u16::try_from(state >> 16).unwrap_or(0)) / 32768.0
    };
    (0..len)
        .map(|_| Vec3::new(next(), next(), next()))
        .collect()
}

/// Denoises `width` x `height` repeatedly; `use_sig` passes the path signature.
fn run(bencher: divan::Bencher, width: u32, height: u32, use_sig: bool) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let n_d = if use_sig { 1.76 } else { 0.0 };
    let guide = generate_guide_buffers_with_index(width, height, &camera, &planes, n_d);
    let color = noisy_color(width as usize * height as usize);
    let inputs = GBuffers {
        color: &color,
        depth: &guide.depth,
        normal: &guide.normal,
        facet_id: &guide.facet_id,
        path_sig: use_sig.then_some(guide.path_sig.as_slice()),
        width: width as usize,
        height: height as usize,
        spp: 16,
    };
    let params = AtrousParams::default();
    let mut denoiser = AtrousDenoiser::new();
    let mut out = Vec::new();
    bencher.bench_local(|| {
        denoiser.denoise_into(black_box(&inputs), black_box(&params), &mut out);
        black_box(&out);
    });
}

/// 800x600, no signature.
#[divan::bench(sample_count = 15)]
fn w800(bencher: divan::Bencher) {
    run(bencher, 800, 600, false);
}

/// 800x600, with the path signature.
#[divan::bench(sample_count = 15)]
fn w800_sig(bencher: divan::Bencher) {
    run(bencher, 800, 600, true);
}

/// 1920x1080, no signature.
#[divan::bench(sample_count = 8)]
fn w1080p(bencher: divan::Bencher) {
    run(bencher, 1920, 1080, false);
}

/// 1920x1080, with the path signature.
#[divan::bench(sample_count = 8)]
fn w1080p_sig(bencher: divan::Bencher) {
    run(bencher, 1920, 1080, true);
}

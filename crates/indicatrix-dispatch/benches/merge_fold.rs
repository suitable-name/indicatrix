//! Deterministic chunk merge, `Merger::add` (BLD-2; finding NET-1 in
//! `review_perf_2026-10-12_fable.md`, section 5, table row 8: coordinator non-render tail
//! per chunk, 50-300 ms before, 20-50 ms less after).
//!
//! Each iteration adds two consecutive full-frame chunks into a fresh merger: the first
//! becomes the merged prefix (a move), the second folds into it with `add_assign`. Both
//! pay the invalid-pixel scan and the mutex. The chunk buffers are built outside the timed
//! region. At 1080p one buffer is 24 MB (12 bytes x 2 073 600 pixels), at 4K 100 MB; expect
//! a few ms (1080p) to a few tens of ms (4K) per add, memory-bandwidth bound.
//!
//! Run: `cargo bench -p indicatrix-dispatch --bench merge_fold`.

use divan::black_box;
use glam::Vec3;
use indicatrix_dispatch::Merger;

fn main() {
    divan::main();
}

/// 1080p pixel count.
const PIXELS_1080P: usize = 1920 * 1080;
/// 4K (UHD) pixel count.
const PIXELS_4K: usize = 3840 * 2160;
/// Samples per chunk.
const CHUNK: u32 = 64;

/// A finite, non-negative chunk buffer (the merge zeroes anything else).
fn chunk(pixels: usize, seed: f32) -> Vec<Vec3> {
    (0..pixels)
        .map(|i| {
            let t = (i % 1024) as f32 * (1.0 / 1024.0);
            Vec3::new(t + seed, 0.5f32.mul_add(t, seed), 0.25 + seed)
        })
        .collect()
}

/// Folds two consecutive chunks of `pixels` pixels into a fresh merger.
fn run(bencher: divan::Bencher, pixels: usize) {
    bencher
        .with_inputs(|| {
            (
                Merger::new(pixels, 0),
                chunk(pixels, 1.0),
                chunk(pixels, 2.0),
            )
        })
        .bench_values(|(merger, first, second)| {
            let _ = black_box(merger.add(0, CHUNK, first));
            let _ = black_box(merger.add(CHUNK, CHUNK, second));
            merger
        });
}

/// Two chunks, 1080p.
#[divan::bench(sample_count = 20)]
fn add_fold_1080p(bencher: divan::Bencher) {
    run(bencher, PIXELS_1080P);
}

/// Two chunks, 4K.
#[divan::bench(sample_count = 8)]
fn add_fold_4k(bencher: divan::Bencher) {
    run(bencher, PIXELS_4K);
}

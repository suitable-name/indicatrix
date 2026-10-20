//! Centre-first base-2/base-3 Halton taps, fixed across all pixels and backends.

use crate::renderer::engine::RayOptions;

const fn radical_inverse(mut index: u32, base: u32) -> f32 {
    let reciprocal = 1.0 / base as f32;
    let mut place = reciprocal;
    let mut value = 0.0;
    while index != 0 {
        value += (index % base) as f32 * place;
        index /= base;
        place *= reciprocal;
    }
    value
}

const fn centred(value: f32) -> f32 {
    let shifted = value + 0.5;
    (if shifted >= 1.0 {
        shifted - 1.0
    } else {
        shifted
    }) - 0.5
}

const fn tap(index: u32) -> (f32, f32) {
    (
        centred(radical_inverse(index, 2)),
        centred(radical_inverse(index, 3)),
    )
}

/// Shared directly with shader preparation; no per-frame random rotation.
pub(super) const OFFSETS: [(f32, f32); 16] = [
    tap(0),
    tap(1),
    tap(2),
    tap(3),
    tap(4),
    tap(5),
    tap(6),
    tap(7),
    tap(8),
    tap(9),
    tap(10),
    tap(11),
    tap(12),
    tap(13),
    tap(14),
    tap(15),
];

/// Absolute tap indices repeat only after the configured, sanitized tap count.
#[must_use]
pub fn aa_offset(taps: u8, sample: u32) -> (f32, f32) {
    let count = RayOptions {
        aa_taps: taps,
        fire: false,
    }
    .sanitized()
    .aa_taps;
    OFFSETS[(sample % u32::from(count)) as usize]
}

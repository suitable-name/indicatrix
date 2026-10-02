//! GENERATED FILE: the adaptive payload-encoding matrix. Do not edit by hand.
//!
//! Regenerate from live measurements (PowerShell, repo root) and review the diff:
//!
//! ```text
//! cargo run -p indicatrix-worker --release --example payload_codec_bench -- --large --emit-matrix --matrix-out crates/indicatrix-net/src/messages/encoding_matrix.rs
//! ```
//!
//! then format it with `rustfmt --edition 2024 <this file>`.
//!
//! Generator settings of this revision: model `serial` (compress + transfer + decompress,
//! summed), cpu-share 1.0, tier ladder 50/100/300/1000/2500/10000 Mbit/s. Rows are size
//! classes by RAW payload bytes (small < 1 MiB, medium < 16 MiB, large >= 16 MiB), columns
//! are the bandwidth tiers; every payload cell is an ordered preference list that ends in
//! `Raw`.
//!
//! Provenance of this revision: the measured serial-model winners of three owner
//! benchmark runs at 100, 300 and 1000 Mbit/s over sparse, dense, converged and delta
//! frames at 256x256 (small), 1024x1024 (medium) and 3840x2160 (large). Where the four
//! data kinds disagreed the cell takes the codec most of them preferred (the per-case
//! winners were available, not the per-codec geometric means). EXTRAPOLATED from the cost
//! model rather than measured: every cell of the 50, 2500 and 10000 Mbit/s tiers (slow
//! link: zstd 3, the speed ladder of the owner data; fast links: LZ4 while the saved
//! transfer still exceeds the codec cost, then `Raw`), and the display column for 2500 and
//! 10000 Mbit/s (PNG compresses slower than those links move raw RGBA8).

use super::{DisplayEncoding, PayloadEncoding};

/// The tested bandwidth ladder in Mbit/s, ascending.
pub const TIERS_MBPS: [f64; 6] = [50.0, 100.0, 300.0, 1000.0, 2500.0, 10000.0];

/// Payload encoding preference per size class (small, medium, large) and bandwidth tier.
pub const PAYLOAD_MATRIX: [[&[PayloadEncoding]; 6]; 3] = [
    // small
    [
        &[
            PayloadEncoding::ShuffleZstd { level: 3 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::Raw,
        ],
        &[PayloadEncoding::Raw],
    ],
    // medium
    [
        &[
            PayloadEncoding::ShuffleZstd { level: 3 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::Raw,
        ],
        &[PayloadEncoding::Raw],
    ],
    // large
    [
        &[
            PayloadEncoding::ShuffleZstd { level: 3 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleZstd { level: 3 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::Raw,
        ],
        &[
            PayloadEncoding::ShuffleLz4,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::Raw,
        ],
        &[PayloadEncoding::Raw],
    ],
];

/// Display-frame encoding per size class (small, medium, large) and bandwidth tier.
pub const DISPLAY_MATRIX: [[DisplayEncoding; 6]; 3] = [
    // small
    [
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Rgba8,
        DisplayEncoding::Rgba8,
    ],
    // medium
    [
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Rgba8,
        DisplayEncoding::Rgba8,
    ],
    // large
    [
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Png,
        DisplayEncoding::Rgba8,
        DisplayEncoding::Rgba8,
    ],
];

//! Tests for [`super::matrix`]: the size-class boundaries and the structural invariants the
//! generated matrix must keep after every regeneration.

use super::{BandwidthTier, EncodingMatrix, MEDIUM_LIMIT_BYTES, SMALL_LIMIT_BYTES, SizeClass};
use crate::messages::{DisplayEncoding, PayloadEncoding};

/// How much CPU time per byte an encoding costs, for ordering: Raw < LZ4 < zstd by level.
fn cost_rank(e: PayloadEncoding) -> u32 {
    match e {
        PayloadEncoding::Raw => 0,
        PayloadEncoding::ShuffleLz4 => 1,
        PayloadEncoding::ShuffleZstd { level } => 1 + u32::from(level),
    }
}

#[test]
fn size_classes_split_at_one_and_sixteen_mebibytes() {
    assert_eq!(SizeClass::of_payload_bytes(0), SizeClass::Small);
    assert_eq!(
        SizeClass::of_payload_bytes(SMALL_LIMIT_BYTES - 1),
        SizeClass::Small
    );
    assert_eq!(
        SizeClass::of_payload_bytes(SMALL_LIMIT_BYTES),
        SizeClass::Medium
    );
    assert_eq!(
        SizeClass::of_payload_bytes(MEDIUM_LIMIT_BYTES - 1),
        SizeClass::Medium
    );
    assert_eq!(
        SizeClass::of_payload_bytes(MEDIUM_LIMIT_BYTES),
        SizeClass::Large
    );
    assert_eq!(SizeClass::of_payload_bytes(usize::MAX), SizeClass::Large);
}

#[test]
fn the_benchmark_sizes_land_in_their_classes() {
    assert_eq!(SizeClass::of_pixels(128 * 128), SizeClass::Small);
    assert_eq!(SizeClass::of_pixels(256 * 256), SizeClass::Small);
    assert_eq!(SizeClass::of_pixels(512 * 512), SizeClass::Medium);
    assert_eq!(SizeClass::of_pixels(1024 * 1024), SizeClass::Medium);
    assert_eq!(SizeClass::of_pixels(3840 * 2160), SizeClass::Large);
    assert_eq!(SizeClass::of_pixels(usize::MAX), SizeClass::Large);
    for (i, c) in SizeClass::ALL.into_iter().enumerate() {
        assert_eq!(c.index(), i);
    }
}

#[test]
fn every_cell_is_a_nonempty_list_ending_in_raw_without_repeats() {
    let matrix = EncodingMatrix::GENERATED;
    for class in SizeClass::ALL {
        for tier in BandwidthTier::all() {
            let cell = matrix.payload_preference(class, tier);
            let at = format!("{} @ {} Mbit/s", class.name(), tier.mbps());
            assert!(!cell.is_empty(), "{at}: empty");
            assert_eq!(
                cell.last(),
                Some(&PayloadEncoding::Raw),
                "{at}: must end in Raw"
            );
            for (i, e) in cell.iter().enumerate() {
                assert!(
                    !cell[..i].iter().any(|p| p.same_family(*e)),
                    "{at}: {e:?} repeats a family"
                );
                if let PayloadEncoding::ShuffleZstd { level } = e {
                    assert!((1..=22).contains(level), "{at}: zstd level {level}");
                }
            }
        }
    }
}

/// Higher tiers never lead with a slower-compressing codec than lower tiers of the same
/// class: a faster link has less transfer time to win back, so the codec cost may only
/// shrink. The owner measurements satisfy this; a regeneration that does not should be
/// inspected (noise between near-tied cells) before it is accepted.
#[test]
fn faster_tiers_never_lead_with_a_slower_codec() {
    let matrix = EncodingMatrix::GENERATED;
    for class in SizeClass::ALL {
        let ranks: Vec<u32> = BandwidthTier::all()
            .into_iter()
            .map(|t| cost_rank(matrix.payload_preference(class, t)[0]))
            .collect();
        assert!(
            ranks.windows(2).all(|w| w[0] >= w[1]),
            "{}: lead codec cost by ascending tier is {ranks:?}",
            class.name()
        );
    }
}

#[test]
fn display_goes_from_png_to_raw_and_never_back() {
    let matrix = EncodingMatrix::GENERATED;
    for class in SizeClass::ALL {
        let cells: Vec<DisplayEncoding> = BandwidthTier::all()
            .into_iter()
            .map(|t| matrix.display_choice(class, t))
            .collect();
        let first_raw = cells.iter().position(|c| *c == DisplayEncoding::Rgba8);
        if let Some(at) = first_raw {
            assert!(
                cells[at..].iter().all(|c| *c == DisplayEncoding::Rgba8),
                "{}: {cells:?}",
                class.name()
            );
        }
    }
}

#[test]
fn the_slowest_tier_compresses() {
    let matrix = EncodingMatrix::default();
    for class in SizeClass::ALL {
        let lead = matrix.payload_preference(class, BandwidthTier::LOWEST)[0];
        assert_ne!(
            lead,
            PayloadEncoding::Raw,
            "{}: slowest link sends raw",
            class.name()
        );
    }
}

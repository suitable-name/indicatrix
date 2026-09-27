//! Tests for [`Merger`].

use super::*;

fn chunk(value: f32) -> Vec<Vec3> {
    vec![Vec3::splat(value); 2]
}

#[test]
fn chunks_arriving_out_of_order_fold_in_start_order() {
    // Values chosen so float addition order matters: 1e8 + 1 + 1 - 1e8 differs
    // from 1e8 - 1e8 + 1 + 1 in f32.
    let huge = chunk(1.0e8);
    let one = chunk(1.0);
    let another_one = chunk(1.0);
    let minus_huge = chunk(-1.0e8);
    let in_order = Merger::new(2, 10);
    in_order.add(10, 3, huge.clone()).unwrap();
    in_order.add(13, 1, one.clone()).unwrap();
    in_order.add(14, 2, another_one.clone()).unwrap();
    in_order.add(16, 4, minus_huge.clone()).unwrap();
    let shuffled = Merger::new(2, 10);
    shuffled.add(16, 4, minus_huge).unwrap();
    shuffled.add(13, 1, one).unwrap();
    assert_eq!(shuffled.total(), 5, "parked chunks count immediately");
    shuffled.add(14, 2, another_one).unwrap();
    shuffled.add(10, 3, huge).unwrap();
    let (ordered_sum, ordered_count) = in_order.into_parts();
    let (shuffled_sum, shuffled_count) = shuffled.into_parts();
    assert_eq!((ordered_count, shuffled_count), (10, 10));
    let bits = |sum: &[Vec3]| {
        sum.iter()
            .flat_map(|p| p.to_array().map(f32::to_bits))
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&ordered_sum), bits(&shuffled_sum));
}

#[test]
fn overlapping_or_misshapen_chunks_are_refused_and_not_counted() {
    let merger = Merger::new(2, 0);
    merger.add(0, 4, chunk(1.0)).unwrap();
    merger.add(10, 5, chunk(1.0)).unwrap();
    assert_eq!(
        merger.add(2, 2, chunk(1.0)),
        Err(MergeError::Overlap {
            first_sample: 2,
            done: 2
        })
    );
    assert!(matches!(
        merger.add(12, 1, chunk(1.0)),
        Err(MergeError::Overlap { .. })
    ));
    assert!(matches!(
        merger.add(8, 3, chunk(1.0)),
        Err(MergeError::Overlap { .. })
    ));
    assert_eq!(
        merger.add(4, 1, vec![Vec3::ONE; 3]),
        Err(MergeError::WrongLength {
            expected: 2,
            got: 3
        })
    );
    assert_eq!(
        merger.add(4, 0, Vec::new()),
        Ok(9),
        "an empty chunk is a no-op"
    );
    assert_eq!(merger.total(), 9);
    // The gap [4, 10) fills and the frontier folds everything.
    merger.add(4, 6, chunk(2.0)).unwrap();
    let mut dst = vec![Vec3::ZERO; 2];
    assert_eq!(merger.snapshot_into(&mut dst), 15);
    assert_eq!(dst, vec![Vec3::splat(4.0); 2]);
}

#[test]
fn snapshot_refuses_a_wrong_sized_destination() {
    let merger = Merger::new(2, 0);
    merger.add(0, 1, chunk(1.0)).unwrap();
    assert_eq!(merger.snapshot_into(&mut [Vec3::ZERO; 3]), 0);
}

#[test]
fn an_empty_merger_yields_a_zero_buffer() {
    let (buffer, count) = Merger::new(3, 7).into_parts();
    assert_eq!(buffer, vec![Vec3::ZERO; 3]);
    assert_eq!(count, 0);
}

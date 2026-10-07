//! Tests for [`Merger`].

use super::*;
use std::sync::atomic::{AtomicI64, Ordering};

fn chunk(value: f32) -> Vec<Vec3> {
    vec![Vec3::splat(value); 2]
}

/// A [`ParkedBudget`] test double capping total reserved bytes at `cap` (net of
/// releases), so a test can drive [`Merger::add`] past it and check both the refusal
/// and that a later fold releases the budget back.
#[derive(Debug)]
struct FakeBudget {
    cap: i64,
    used: AtomicI64,
}

impl FakeBudget {
    fn new(cap: i64) -> Arc<Self> {
        Arc::new(Self {
            cap,
            used: AtomicI64::new(0),
        })
    }
}

impl ParkedBudget for FakeBudget {
    fn reserve(&self, bytes: u64) -> bool {
        let bytes = i64::try_from(bytes).expect("test byte counts fit in i64");
        let prev = self.used.fetch_add(bytes, Ordering::SeqCst);
        if prev + bytes > self.cap {
            self.used.fetch_sub(bytes, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn release(&self, bytes: u64) {
        let bytes = i64::try_from(bytes).expect("test byte counts fit in i64");
        self.used.fetch_sub(bytes, Ordering::SeqCst);
    }
}

#[test]
fn chunks_arriving_out_of_order_fold_in_start_order() {
    // Chosen so the two fold orders give DIFFERENT, exactly known f32 results, not
    // merely "some value both variants happen to agree on". Near 1e8 an f32 step is
    // 8, so folding in START order (ascending first_sample: 1e8, 5, 5) gives
    // 0 + 1e8 = 1e8, then 1e8 + 5 rounds up to 1e8 + 8, then (1e8 + 8) + 5 = 1e8 + 13
    // rounds to 1e8 + 16 == 100_000_016.0 exactly. Folding in ARRIVAL order instead
    // (the bug this guards: merging as chunks are handed to `add` rather than by
    // first_sample) would compute (0 + 5 + 5) + 1e8 = 1e8 + 10, which rounds to
    // 1e8 + 8 == 100_000_008.0. A test that only compared two Mergers to each other
    // can't tell these apart: `Merger`'s frontier-based fold always merges by
    // first_sample regardless of call order, so both variants always agree -- the
    // point of asserting against the literal below is to also catch a regression that
    // folds by arrival order. Every value is non-negative because `Merger::add` zeroes
    // negative radiance.
    let huge = chunk(1.0e8);
    let five = chunk(5.0);
    let another_five = chunk(5.0);
    let in_order = Merger::new(2, 10);
    in_order.add(10, 1, huge.clone()).unwrap();
    in_order.add(11, 1, five.clone()).unwrap();
    in_order.add(12, 1, another_five.clone()).unwrap();
    // Arrival order [12, 11, 10]: the two "5" chunks are added first (parked, ahead
    // of the frontier at 10), then the huge one releases the whole run.
    let shuffled = Merger::new(2, 10);
    shuffled.add(12, 1, another_five).unwrap();
    shuffled.add(11, 1, five).unwrap();
    assert_eq!(shuffled.total(), 2, "parked chunks count immediately");
    shuffled.add(10, 1, huge).unwrap();
    let (ordered_sum, ordered_count) = in_order.into_parts();
    let (shuffled_sum, shuffled_count) = shuffled.into_parts();
    assert_eq!((ordered_count, shuffled_count), (3, 3));
    assert_eq!(ordered_sum, vec![Vec3::splat(100_000_016.0); 2]);
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

/// A chunk that would have to park stays refused (not counted) once the budget
/// is exhausted, and a chunk that fills the gap at the frontier -- folding the backlog
/// away -- both never needs budget of its own AND releases what the folded chunk(s)
/// had reserved, so a later chunk can park again.
#[test]
fn parking_past_the_budget_is_refused_and_releases_once_the_frontier_catches_up() {
    // Each 2-pixel chunk costs 2 * size_of::<Vec3>() = 24 bytes; a cap of 24 admits
    // exactly one parked chunk at a time.
    let budget = FakeBudget::new(24);
    let merger = Merger::new(2, 0).with_parked_budget(Arc::clone(&budget) as Arc<dyn ParkedBudget>);

    // Parked (sample 0 hasn't arrived yet): the only parked chunk, charged 24 bytes.
    assert_eq!(merger.add(2, 1, chunk(1.0)), Ok(1));
    assert_eq!(budget.used.load(Ordering::SeqCst), 24);

    // A second parked chunk needs another 24 bytes -- over the cap, refused and not
    // counted.
    assert_eq!(
        merger.add(4, 1, chunk(1.0)),
        Err(MergeError::ParkedBudgetExceeded { bytes: 24 })
    );
    assert_eq!(merger.total(), 1, "the refused chunk is not counted");

    // Filling the gap at the frontier folds the parked chunk (and itself) away without
    // ever needing budget of its own, and releases the folded chunk's 24 bytes.
    assert_eq!(merger.add(0, 2, chunk(1.0)), Ok(3));
    assert_eq!(budget.used.load(Ordering::SeqCst), 0);

    // The budget is free again: a later chunk can park.
    assert_eq!(merger.add(4, 1, chunk(1.0)), Ok(4));
    assert_eq!(budget.used.load(Ordering::SeqCst), 24);
}

/// Dropping a merger that still has parked chunks (a cancelled run) returns their
/// charge to the shared budget.
#[test]
fn dropping_a_merger_releases_its_parked_bytes() {
    let budget = FakeBudget::new(1000);
    let merger = Merger::new(2, 0).with_parked_budget(Arc::clone(&budget) as Arc<dyn ParkedBudget>);
    assert_eq!(merger.add(2, 1, chunk(1.0)), Ok(1));
    assert_eq!(merger.add(4, 1, chunk(1.0)), Ok(2));
    assert_eq!(budget.used.load(Ordering::SeqCst), 48);
    drop(merger);
    assert_eq!(budget.used.load(Ordering::SeqCst), 0);
}

/// `into_parts` releases the parked charge, and the drop that follows (inside it) must
/// not release it again; a second merger's charge stays untouched.
#[test]
fn into_parts_then_drop_does_not_double_release() {
    let budget = FakeBudget::new(1000);
    let shared = Arc::clone(&budget) as Arc<dyn ParkedBudget>;
    let other = Merger::new(2, 0).with_parked_budget(Arc::clone(&shared));
    assert_eq!(other.add(2, 1, chunk(1.0)), Ok(1));
    let merger = Merger::new(2, 0).with_parked_budget(shared);
    assert_eq!(merger.add(2, 1, chunk(1.0)), Ok(1));
    assert_eq!(budget.used.load(Ordering::SeqCst), 48);
    let (_, count) = merger.into_parts();
    assert_eq!(count, 1);
    assert_eq!(budget.used.load(Ordering::SeqCst), 24);
    drop(other);
    assert_eq!(budget.used.load(Ordering::SeqCst), 0);
}

/// A chunk that folded already released its charge; dropping afterwards releases only
/// what is still parked.
#[test]
fn folding_then_drop_does_not_double_release() {
    let budget = FakeBudget::new(1000);
    let shared = Arc::clone(&budget) as Arc<dyn ParkedBudget>;
    let other = Merger::new(2, 0).with_parked_budget(Arc::clone(&shared));
    assert_eq!(other.add(2, 1, chunk(1.0)), Ok(1));
    let merger = Merger::new(2, 0).with_parked_budget(shared);
    assert_eq!(merger.add(2, 1, chunk(1.0)), Ok(1));
    assert_eq!(merger.add(0, 2, chunk(1.0)), Ok(3));
    assert_eq!(
        budget.used.load(Ordering::SeqCst),
        24,
        "only `other` is parked"
    );
    drop(merger);
    assert_eq!(budget.used.load(Ordering::SeqCst), 24);
    drop(other);
    assert_eq!(budget.used.load(Ordering::SeqCst), 0);
}

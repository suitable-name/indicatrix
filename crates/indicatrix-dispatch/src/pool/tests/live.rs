//! Lanes joining and leaving a running epoch: [`LaneFeed`] and [`WorkerLane::lost`].

use super::{
    fake::{DyingLane, Failure, FakeLane, Values, scene, value},
    *,
};

/// Hands out `late` once `watch` has traced `after` samples.
struct LateFeed {
    watch: Arc<FakeLane>,
    after: usize,
    late: Mutex<Option<Arc<dyn WorkerLane>>>,
}

impl LaneFeed for LateFeed {
    fn next(&self, _first_index: usize, wait: Duration) -> Vec<(Arc<dyn WorkerLane>, RateModel)> {
        if self.watch.traced().len() >= self.after
            && let Some(lane) = self.late.lock().unwrap().take()
        {
            return vec![(lane, RateModel::new(1000.0))];
        }
        std::thread::sleep(wait.min(5 * MS));
        Vec::new()
    }
}

fn run_fed(
    pool: &LanePool,
    range: SampleRange,
    feed: &dyn LaneFeed,
) -> (PoolStatus, Merger, Vec<PoolEvent>) {
    let merger = Merger::new(4, range.first_sample);
    let events = Mutex::new(Vec::new());
    let status = pool
        .run_into_fed(
            &scene(2, 2),
            range,
            &merger,
            &CancelToken::new(),
            &|event| events.lock().unwrap().push(event),
            Some(feed),
        )
        .expect("a fresh merger for this image");
    (status, merger, events.into_inner().unwrap())
}

/// The merged buffer equals the exact reference over `samples`, each index once.
fn assert_exact(merger: &Merger, samples: &mut [u32], range: SampleRange) {
    samples.sort_unstable();
    for pair in samples.windows(2) {
        assert_ne!(pair[0], pair[1], "sample {} was traced twice", pair[0]);
    }
    let mut snapshot = vec![Vec3::ZERO; 4];
    assert_eq!(merger.snapshot_into(&mut snapshot), samples.len() as u32);
    let expected: Vec<u32> = (range.first_sample..range.end()).collect();
    if samples.len() == expected.len() {
        assert_eq!(samples, expected.as_slice());
    }
    let mut reference = vec![Vec3::ZERO; 4];
    for &sample in &*samples {
        for (pixel, dst) in reference.iter_mut().enumerate() {
            *dst += value(Values::Integer, pixel as u32, sample);
        }
    }
    assert_eq!(snapshot, reference);
}

#[test]
fn a_lane_added_after_some_chunks_joins_the_epoch_and_every_sample_is_traced_once() {
    let first = fake("first", Values::Integer, MS, Failure::Never);
    let late = fake("late", Values::Integer, Duration::ZERO, Failure::Never);
    let pool = pool_of(config(ChunkPolicy::fixed(10)), std::slice::from_ref(&first));
    let feed = LateFeed {
        watch: Arc::clone(&first),
        after: 30,
        late: Mutex::new(Some(Arc::clone(&late) as Arc<dyn WorkerLane>)),
    };
    let range = SampleRange::new(0, 300);
    let (status, merger, events) = run_fed(&pool, range, &feed);
    assert_eq!(status, PoolStatus::Complete);
    assert!(events.contains(&PoolEvent::LaneStarted { lane: 1 }));
    assert!(!late.traced().is_empty(), "the late lane received chunks");
    assert!(
        first.traced().len() >= 30,
        "the first lane had traced its chunks before"
    );
    assert_eq!(pool.lane_count(), 2, "the late lane is registered");
    assert!(pool.rate(1).is_some_and(|rate| rate.is_calibrated()));
    let mut all: Vec<u32> = first.traced().into_iter().chain(late.traced()).collect();
    assert_exact(&merger, &mut all, range);
}

#[test]
fn a_lane_offered_after_the_run_finished_is_never_started() {
    let first = fake("first", Values::Integer, Duration::ZERO, Failure::Never);
    let late = fake("late", Values::Integer, Duration::ZERO, Failure::Never);
    let pool = pool_of(config(ChunkPolicy::fixed(10)), std::slice::from_ref(&first));
    let feed = LateFeed {
        watch: Arc::clone(&first),
        // Only reachable once the run is over: the range is 40 samples.
        after: 1_000,
        late: Mutex::new(Some(Arc::clone(&late) as Arc<dyn WorkerLane>)),
    };
    let range = SampleRange::new(0, 40);
    let (status, merger, _) = run_fed(&pool, range, &feed);
    assert_eq!(status, PoolStatus::Complete);
    assert_eq!(late.traced(), Vec::<u32>::new());
    assert_eq!(pool.lane_count(), 1);
    let mut all = first.traced();
    assert_exact(&merger, &mut all, range);
}

#[test]
fn a_lost_lane_is_removed_at_once_and_its_tail_is_traced_exactly_once() {
    let steady = fake(
        "steady",
        Values::Integer,
        Duration::from_micros(300),
        Failure::Never,
    );
    let dying = Arc::new(DyingLane::new(Duration::ZERO, 2, 3));
    let mut pool = LanePool::new(config(ChunkPolicy::fixed(10)));
    pool.add_lane(
        Arc::clone(&steady) as Arc<dyn WorkerLane>,
        RateModel::new(1000.0),
    );
    pool.add_lane(
        Arc::clone(&dying) as Arc<dyn WorkerLane>,
        RateModel::new(1000.0),
    );
    let range = SampleRange::new(0, 600);
    let (status, merger, events) = run_fed(&pool, range, &NoLanes);
    assert_eq!(status, PoolStatus::Complete);
    assert!(events.contains(&PoolEvent::LaneRemoved { lane: 1 }));
    assert_eq!(dying.chunks(), 3, "no retry after the connection was lost");
    assert!(
        !events.iter().any(|event| matches!(
            event,
            PoolEvent::LaneRetired { .. } | PoolEvent::LaneFailed { lane: 1, .. }
        )),
        "removal is neither a retry nor a retirement"
    );
    let mut all: Vec<u32> = steady.traced().into_iter().chain(dying.traced()).collect();
    assert_exact(&merger, &mut all, range);
}

#[test]
fn the_last_lane_is_never_removed_so_only_an_empty_pool_fails_the_run() {
    let a = Arc::new(DyingLane::new(Duration::ZERO, 1, 2));
    let b = Arc::new(DyingLane::new(Duration::ZERO, 1, 2));
    let mut pool = LanePool::new(config(ChunkPolicy::fixed(10)));
    for lane in [&a, &b] {
        pool.add_lane(
            Arc::clone(lane) as Arc<dyn WorkerLane>,
            RateModel::new(1000.0),
        );
    }
    let range = SampleRange::new(0, 500);
    let (status, _, events) = run_fed(&pool, range, &NoLanes);
    assert!(matches!(status, PoolStatus::LanesExhausted { missing } if missing > 0));
    let count = |wanted: fn(&PoolEvent) -> bool| events.iter().filter(|e| wanted(e)).count();
    assert_eq!(count(|e| matches!(e, PoolEvent::LaneRemoved { .. })), 1);
    assert_eq!(count(|e| matches!(e, PoolEvent::LaneRetired { .. })), 1);
}

#[test]
fn a_replacement_fed_in_after_a_removal_finishes_the_run() {
    let steady = fake(
        "steady",
        Values::Integer,
        Duration::from_micros(300),
        Failure::Never,
    );
    let dying = Arc::new(DyingLane::new(Duration::ZERO, 1, 2));
    let replacement = fake("again", Values::Integer, Duration::ZERO, Failure::Never);
    let mut pool = LanePool::new(config(ChunkPolicy::fixed(10)));
    pool.add_lane(
        Arc::clone(&steady) as Arc<dyn WorkerLane>,
        RateModel::new(1000.0),
    );
    pool.add_lane(
        Arc::clone(&dying) as Arc<dyn WorkerLane>,
        RateModel::new(1000.0),
    );
    let feed = LateFeed {
        watch: Arc::clone(&steady),
        after: 20,
        late: Mutex::new(Some(Arc::clone(&replacement) as Arc<dyn WorkerLane>)),
    };
    let range = SampleRange::new(0, 400);
    let (status, merger, events) = run_fed(&pool, range, &feed);
    assert_eq!(status, PoolStatus::Complete);
    assert!(events.contains(&PoolEvent::LaneRemoved { lane: 1 }));
    assert_ne!(replacement.traced(), Vec::<u32>::new());
    let mut all: Vec<u32> = steady
        .traced()
        .into_iter()
        .chain(dying.traced())
        .chain(replacement.traced())
        .collect();
    assert_exact(&merger, &mut all, range);
}

/// A feed with nothing to add.
struct NoLanes;

impl LaneFeed for NoLanes {
    fn next(&self, _first_index: usize, wait: Duration) -> Vec<(Arc<dyn WorkerLane>, RateModel)> {
        std::thread::sleep(wait.min(5 * MS));
        Vec::new()
    }
}

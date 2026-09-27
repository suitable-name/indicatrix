//! [`LanePool`] tests with in-process fake lanes (see [`fake`]).

mod fake;

use super::*;
use fake::{Failure, FakeLane, Values, chunk_sum, scene, value};
use std::{sync::Mutex, time::Instant};

const MS: Duration = Duration::from_millis(1);

/// Short pauses so failure tests run fast; pause after every failure, retire after 3.
fn config(policy: ChunkPolicy) -> PoolConfig {
    PoolConfig {
        policy,
        pause_after_failures: 1,
        retire_after_failures: 3,
        backoff_initial: Duration::from_millis(5),
        backoff_max: Duration::from_millis(20),
    }
}

fn fake(name: &str, values: Values, per_sample: Duration, failure: Failure) -> Arc<FakeLane> {
    Arc::new(FakeLane::new(name, values, per_sample, failure))
}

fn pool_of(config: PoolConfig, lanes: &[Arc<FakeLane>]) -> LanePool {
    let mut pool = LanePool::new(config);
    for lane in lanes {
        pool.add_lane(
            Arc::clone(lane) as Arc<dyn WorkerLane>,
            RateModel::new(1000.0),
        );
    }
    pool
}

/// Runs `pool` and records every event.
fn run_recording(
    pool: &LanePool,
    scene: &SceneState,
    range: SampleRange,
    cancel: &CancelToken,
) -> (PoolOutcome, Vec<PoolEvent>) {
    let events = Mutex::new(Vec::new());
    let outcome = pool.run(scene, range, cancel, &|event| {
        events.lock().unwrap().push(event);
    });
    (outcome, events.into_inner().unwrap())
}

/// Every sample any lane delivered, sorted; panics on a duplicate.
fn traced_once(lanes: &[Arc<FakeLane>]) -> Vec<u32> {
    let mut all: Vec<u32> = lanes.iter().flat_map(|lane| lane.traced()).collect();
    all.sort_unstable();
    for pair in all.windows(2) {
        assert_ne!(pair[0], pair[1], "sample {} was traced twice", pair[0]);
    }
    all
}

/// The exact reference sum over `samples` (integer values: order-independent).
fn reference(pixels: usize, samples: &[u32]) -> Vec<Vec3> {
    let mut sum = vec![Vec3::ZERO; pixels];
    for &sample in samples {
        for (pixel, dst) in sum.iter_mut().enumerate() {
            *dst += value(Values::Integer, pixel as u32, sample);
        }
    }
    sum
}

/// Asserts a complete run: every index of `range` traced exactly once, count exact,
/// merged buffer exactly the reference.
fn assert_complete(
    outcome: &PoolOutcome,
    lanes: &[Arc<FakeLane>],
    range: SampleRange,
    pixels: usize,
) {
    assert_eq!(outcome.status, PoolStatus::Complete);
    assert_eq!(outcome.count, range.samples);
    let traced = traced_once(lanes);
    let expected: Vec<u32> = (range.first_sample..range.end()).collect();
    assert_eq!(traced, expected, "every sample index traced exactly once");
    assert_eq!(outcome.sum, reference(pixels, &traced));
}

fn failures_of(events: &[PoolEvent], lane: usize) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, PoolEvent::LaneFailed { lane: l, .. } if *l == lane))
        .count()
}

#[test]
fn one_lane_traces_every_sample_exactly_once() {
    let lanes = [fake("a", Values::Integer, Duration::ZERO, Failure::Never)];
    let pool = pool_of(config(ChunkPolicy::fixed(9)), &lanes);
    let range = SampleRange::new(5, 200);
    let (outcome, _) = run_recording(&pool, &scene(4, 2), range, &CancelToken::new());
    assert_complete(&outcome, &lanes, range, 8);
}

#[test]
fn two_lanes_of_different_speeds_trace_every_sample_exactly_once() {
    let lanes = [
        fake("fast", Values::Integer, Duration::ZERO, Failure::Never),
        fake("slow", Values::Integer, MS, Failure::Never),
    ];
    let pool = pool_of(config(ChunkPolicy::fixed(10)), &lanes);
    let range = SampleRange::new(0, 300);
    let (outcome, _) = run_recording(&pool, &scene(3, 3), range, &CancelToken::new());
    assert_complete(&outcome, &lanes, range, 9);
}

#[test]
fn four_lanes_with_rate_sized_chunks_trace_every_sample_exactly_once() {
    let lanes = [
        fake("a", Values::Integer, Duration::ZERO, Failure::Never),
        fake(
            "b",
            Values::Integer,
            Duration::from_micros(200),
            Failure::Never,
        ),
        fake("c", Values::Integer, MS, Failure::Never),
        fake("d", Values::Integer, 2 * MS, Failure::Never),
    ];
    let policy = ChunkPolicy {
        target: Duration::from_millis(15),
        min_samples: 2,
        max_samples: 50,
        calibration_samples: 3,
    };
    let pool = pool_of(config(policy), &lanes);
    let range = SampleRange::new(100, 600);
    let (outcome, events) = run_recording(&pool, &scene(2, 2), range, &CancelToken::new());
    assert_complete(&outcome, &lanes, range, 4);
    let finished = events
        .iter()
        .filter(|event| matches!(event, PoolEvent::LaneFinished { .. }))
        .count();
    assert_eq!(finished, 4);
}

#[test]
fn a_lane_failing_mid_chunk_has_its_remainder_retraced() {
    let lanes = [
        fake(
            "flaky",
            Values::Integer,
            Duration::ZERO,
            Failure::First {
                chunks: 2,
                after: 3,
            },
        ),
        fake("steady", Values::Integer, MS, Failure::Never),
    ];
    let pool = pool_of(config(ChunkPolicy::fixed(10)), &lanes);
    let range = SampleRange::new(0, 150);
    let (outcome, events) = run_recording(&pool, &scene(2, 3), range, &CancelToken::new());
    assert_complete(&outcome, &lanes, range, 6);
    assert_eq!(failures_of(&events, 0), 2);
    for event in &events {
        if let PoolEvent::LaneFailed {
            returned, pause, ..
        } = event
        {
            assert!(!returned.is_empty());
            assert!(
                pause.is_some(),
                "two failures stay below the retirement limit"
            );
        }
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PoolEvent::LaneRetired { .. }))
    );
}

#[test]
fn a_lane_that_keeps_failing_is_retired_and_the_others_finish() {
    let lanes = [
        fake(
            "broken",
            Values::Integer,
            Duration::ZERO,
            Failure::Always { after: 2 },
        ),
        fake(
            "steady",
            Values::Integer,
            Duration::from_micros(300),
            Failure::Never,
        ),
    ];
    let pool = pool_of(config(ChunkPolicy::fixed(8)), &lanes);
    let range = SampleRange::new(0, 240);
    let (outcome, events) = run_recording(&pool, &scene(2, 2), range, &CancelToken::new());
    assert_complete(&outcome, &lanes, range, 4);
    assert!(events.contains(&PoolEvent::LaneRetired {
        lane: 0,
        consecutive_failures: 3
    }));
    assert_eq!(failures_of(&events, 0), 3);
    assert!(
        pool.rate(1).unwrap().is_calibrated(),
        "rate models persist after a run"
    );
}

#[test]
fn a_panicking_lane_is_a_failure_not_a_crash() {
    let lanes = [
        fake("panics", Values::Integer, Duration::ZERO, Failure::Panic),
        // Slow enough that the panicking lane reaches retirement before the run ends.
        fake("steady", Values::Integer, MS, Failure::Never),
    ];
    let pool = pool_of(config(ChunkPolicy::fixed(6)), &lanes);
    let range = SampleRange::new(0, 60);
    let (outcome, events) = run_recording(&pool, &scene(1, 2), range, &CancelToken::new());
    assert_complete(&outcome, &lanes, range, 2);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, PoolEvent::LaneRetired { lane: 0, .. }))
    );
}

#[test]
fn every_lane_retired_reports_the_missing_samples() {
    let lanes = [
        fake(
            "a",
            Values::Integer,
            Duration::ZERO,
            Failure::Always { after: 1 },
        ),
        fake(
            "b",
            Values::Integer,
            Duration::ZERO,
            Failure::Always { after: 1 },
        ),
    ];
    let pool = pool_of(config(ChunkPolicy::fixed(5)), &lanes);
    let range = SampleRange::new(0, 50);
    let (outcome, _) = run_recording(&pool, &scene(2, 1), range, &CancelToken::new());
    let PoolStatus::LanesExhausted { missing } = outcome.status else {
        panic!("expected LanesExhausted, got {:?}", outcome.status);
    };
    assert!(missing > 0);
    assert_eq!(outcome.count + missing, range.samples);
    let traced = traced_once(&lanes);
    assert_eq!(traced.len(), outcome.count as usize);
    assert_eq!(outcome.sum, reference(2, &traced));
}

#[test]
fn cancel_mid_epoch_stops_every_lane_and_keeps_the_count_exact() {
    let lanes = [
        fake("a", Values::Integer, MS, Failure::Never),
        fake("b", Values::Integer, MS, Failure::Never),
        fake("c", Values::Integer, 2 * MS, Failure::Never),
    ];
    let pool = pool_of(config(ChunkPolicy::fixed(4)), &lanes);
    let range = SampleRange::new(0, 100_000);
    let cancel = CancelToken::new();
    let started = Instant::now();
    let outcome = std::thread::scope(|scope| {
        let canceller = cancel.clone();
        scope.spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            canceller.cancel();
        });
        run_recording(&pool, &scene(2, 2), range, &cancel).0
    });
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "cancel must stop the run"
    );
    let PoolStatus::Cancelled { missing } = outcome.status else {
        panic!("expected Cancelled, got {:?}", outcome.status);
    };
    assert_eq!(outcome.count + missing, range.samples);
    let traced = traced_once(&lanes);
    assert_eq!(traced.len(), outcome.count as usize);
    assert_eq!(outcome.sum, reference(4, &traced));
}

/// Same fixed chunk partition, different lane counts and speeds (so different lanes
/// trace different chunks and finish in different orders): the merged buffer is
/// bit-identical, and equals the chunk-start-ordered fold.
#[test]
fn merge_is_bit_identical_regardless_of_lane_timing() {
    let range = SampleRange::new(3, 400);
    let pixels = 6;
    let scene = scene(3, 2);
    let bits = |sum: &[Vec3]| -> Vec<u32> {
        sum.iter()
            .flat_map(|p| p.to_array().map(f32::to_bits))
            .collect()
    };
    let chunks: Vec<SampleRange> = (range.first_sample..range.end())
        .step_by(5)
        .map(|start| SampleRange::new(start, 5.min(range.end() - start)))
        .collect();
    let fold = |order: &mut dyn Iterator<Item = &SampleRange>| {
        let mut merged = vec![Vec3::ZERO; pixels];
        for chunk in order {
            for (dst, src) in
                merged
                    .iter_mut()
                    .zip(chunk_sum(Values::Fractional, pixels, *chunk, chunk.samples))
            {
                *dst += src;
            }
        }
        merged
    };
    let expected = bits(&fold(&mut chunks.iter()));
    assert_ne!(
        expected,
        bits(&fold(&mut chunks.iter().rev())),
        "the test values must make summation order observable"
    );
    let speeds: [&[Duration]; 3] = [
        &[Duration::ZERO],
        &[Duration::ZERO, Duration::from_micros(300), MS],
        &[
            MS,
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_micros(500),
        ],
    ];
    for lane_speeds in speeds {
        let lanes: Vec<_> = lane_speeds
            .iter()
            .map(|&speed| fake("lane", Values::Fractional, speed, Failure::Never))
            .collect();
        let pool = pool_of(config(ChunkPolicy::fixed(5)), &lanes);
        let (outcome, _) = run_recording(&pool, &scene, range, &CancelToken::new());
        assert_eq!(outcome.status, PoolStatus::Complete);
        assert_eq!(bits(&outcome.sum), expected, "{} lanes", lanes.len());
    }
}

#[test]
fn events_report_start_progress_and_finish() {
    let lanes = [fake(
        "solo",
        Values::Integer,
        Duration::ZERO,
        Failure::Never,
    )];
    let pool = pool_of(config(ChunkPolicy::fixed(7)), &lanes);
    let range = SampleRange::new(0, 30);
    let (_, events) = run_recording(&pool, &scene(1, 1), range, &CancelToken::new());
    assert_eq!(events.first(), Some(&PoolEvent::LaneStarted { lane: 0 }));
    assert_eq!(
        events.last(),
        Some(&PoolEvent::LaneFinished {
            lane: 0,
            chunks: 5,
            samples: 30
        })
    );
    let totals: Vec<u32> = events
        .iter()
        .filter_map(|event| match event {
            PoolEvent::ChunkMerged {
                total_done, target, ..
            } => {
                assert_eq!(*target, 30);
                Some(*total_done)
            }
            _ => None,
        })
        .collect();
    assert_eq!(totals, [7, 14, 21, 28, 30]);
    assert_eq!(pool.lane_name(0), Some("solo"));
}

#[test]
fn a_pool_without_lanes_traces_nothing_and_an_empty_range_is_complete() {
    let pool = LanePool::new(PoolConfig::INTERACTIVE);
    let scene = scene(2, 2);
    let (outcome, _) = run_recording(&pool, &scene, SampleRange::new(0, 16), &CancelToken::new());
    assert_eq!(outcome.status, PoolStatus::LanesExhausted { missing: 16 });
    assert_eq!(outcome.count, 0);
    assert_eq!(outcome.sum, vec![Vec3::ZERO; 4]);
    let (empty, _) = run_recording(&pool, &scene, SampleRange::new(9, 0), &CancelToken::new());
    assert_eq!(empty.status, PoolStatus::Complete);
}

#[test]
fn run_into_refuses_a_merger_for_another_image_and_streams_into_a_fresh_one() {
    let lanes = [fake("a", Values::Integer, Duration::ZERO, Failure::Never)];
    let pool = pool_of(config(ChunkPolicy::fixed(4)), &lanes);
    let scene = scene(2, 2);
    let range = SampleRange::new(8, 40);
    let wrong = Merger::new(5, 8);
    let refused = pool.run_into(&scene, range, &wrong, &CancelToken::new(), &|_| {});
    assert_eq!(refused.unwrap_err().merger_pixels, 5);
    let merger = Merger::new(4, 8);
    let status = pool.run_into(&scene, range, &merger, &CancelToken::new(), &|_| {});
    assert_eq!(status, Ok(PoolStatus::Complete));
    let mut snapshot = vec![Vec3::ZERO; 4];
    assert_eq!(merger.snapshot_into(&mut snapshot), 40);
    let samples: Vec<u32> = (8..48).collect();
    assert_eq!(snapshot, reference(4, &samples));
}

#[test]
fn backoff_follows_the_export_schedule() {
    let config = PoolConfig::EXPORT;
    let secs: Vec<u64> = (1..=7).map(|n| config.backoff(n).as_secs()).collect();
    assert_eq!(secs, [0, 15, 30, 60, 120, 120, 120]);
    assert_eq!(
        PoolConfig::INTERACTIVE.backoff(1),
        Duration::from_millis(250)
    );
}

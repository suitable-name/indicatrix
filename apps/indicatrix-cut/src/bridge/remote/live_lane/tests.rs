//! Orchestration tests for [`LiveLane`] + [`LiveEpoch`], with no window and no worker:
//! the "worker" is simulated by applying synthetic `FRAME` deltas to each chunk's own
//! accumulator exactly as the connection thread would.

use super::*;
use crate::bridge::{
    render_thread::RenderContext,
    sample_cursor::tests::{apply_frame, assert_partition},
};
use glam::Vec3;
use indicatrix_net::{
    client::ApplyOutcome,
    messages::{FrameHeader, StreamEvent},
    radiance,
};
use std::time::Duration;

fn t(ms: u64) -> Instant {
    // One fixed origin per test process is enough: only differences matter.
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *ORIGIN.get_or_init(Instant::now) + Duration::from_millis(ms)
}

/// Traces every sample of a chunk (as a well-behaved worker would) and reports `Done`.
fn complete_chunk(lane: &mut LiveLane, chunk: &ChunkRequest, done_at: Instant) -> ChunkEnd {
    apply_frame(
        &chunk.accumulator,
        chunk.request_id,
        chunk.first_sample,
        chunk.samples,
        1.0,
    );
    lane.chunk_done(chunk.request_id, done_at)
        .expect("the chunk is in flight")
}

#[test]
fn chunk_sizing_starts_small_and_then_follows_the_measured_rate() {
    assert_eq!(live_chunk_samples(None), LIVE_FIRST_CHUNK_SAMPLES);
    assert_eq!(live_chunk_samples(Some(100.0)), 150, "1.5 s at 100 spp/s");
    assert_eq!(live_chunk_samples(Some(0.1)), LIVE_CHUNK_MIN_SAMPLES);
    assert_eq!(live_chunk_samples(Some(f64::NAN)), LIVE_CHUNK_MIN_SAMPLES);
    assert_eq!(live_chunk_samples(Some(1e12)), LIVE_CHUNK_MAX_SAMPLES);
}

#[test]
fn a_finished_chunk_updates_the_rate_and_the_next_chunk_is_sized_from_it() {
    let epoch = Arc::new(LiveEpoch::new(1, 1, 10_000));
    let mut lane = LiveLane::new(Arc::clone(&epoch), true);
    let first = lane.next_chunk(1, t(0)).unwrap();
    assert_eq!(
        (first.first_sample, first.samples),
        (0, LIVE_FIRST_CHUNK_SAMPLES)
    );
    assert!(
        lane.next_chunk(2, t(0)).is_none(),
        "one chunk in flight at a time"
    );
    // 8 samples in 100 ms -> 80 spp/s -> the next chunk covers ~1.5 s = 120 samples.
    complete_chunk(&mut lane, &first, t(100));
    let rate = lane.rate().unwrap();
    assert!((rate - 80.0).abs() < 1e-6, "rate {rate}");
    let second = lane.next_chunk(2, t(100)).unwrap();
    assert_eq!((second.first_sample, second.samples), (8, 120));
}

#[test]
fn the_lane_never_claims_past_the_target_and_then_reports_finished() {
    let epoch = Arc::new(LiveEpoch::new(1, 1, 12));
    let mut lane = LiveLane::new(Arc::clone(&epoch), true);
    let a = lane.next_chunk(1, t(0)).unwrap();
    complete_chunk(&mut lane, &a, t(10));
    let b = lane.next_chunk(2, t(10)).unwrap();
    assert_eq!((b.first_sample, b.samples), (8, 4), "clamped to the target");
    complete_chunk(&mut lane, &b, t(20));
    assert!(lane.next_chunk(3, t(20)).is_none());
    assert!(lane.is_finished());
    assert_eq!(epoch.remote_done(), 12);
}

/// A drag mid-chunk: the lane hands back the in-flight id to cancel, nothing of the
/// abandoned chunk is merged, and nothing of it -- not even a late `FRAME` still on the
/// wire -- leaks into the next epoch.
#[test]
fn a_drag_mid_chunk_cancels_and_nothing_leaks_into_the_next_epoch() {
    let ctx = Mutex::new(RenderContext::default());
    let epoch1 = Arc::new(LiveEpoch::new(2, 2, 64));
    {
        let mut c = ctx.lock().unwrap();
        c.remote_active = true;
        c.live_epoch = Some(Arc::clone(&epoch1));
    }
    let mut lane1 = LiveLane::new(Arc::clone(&epoch1), true);
    let chunk1 = lane1.next_chunk(1, t(0)).unwrap();
    apply_frame(&chunk1.accumulator, 1, chunk1.first_sample, 3, 5.0);
    assert_eq!(epoch1.remote_done(), 3);

    // The drag: cancel on the wire, abandon the lane, release the context.
    assert_eq!(
        lane1.abandon(),
        Some(1),
        "the in-flight request must be cancelled"
    );
    ctx.lock().unwrap().release_remote();
    assert_eq!(
        epoch1.remote_done(),
        0,
        "an abandoned chunk is never merged"
    );
    {
        let c = ctx.lock().unwrap();
        let (remote_active, has_epoch, dirty) = (c.remote_active, c.live_epoch.is_some(), c.dirty);
        drop(c);
        assert!(!remote_active);
        assert!(!has_epoch);
        assert!(dirty, "local restarts from a clean buffer");
    }
    // A late reply for the abandoned chunk can no longer move the lane.
    assert!(lane1.chunk_done(1, t(5)).is_none());
    assert!(lane1.chunk_failed(1).is_none());

    // The next settle: a brand-new epoch and lane.
    let epoch2 = Arc::new(LiveEpoch::new(2, 2, 64));
    let mut lane2 = LiveLane::new(Arc::clone(&epoch2), true);
    let chunk2 = lane2.next_chunk(2, t(10)).unwrap();
    // A late FRAME for request 1 reaching the new chunk's accumulator is stale.
    let buf = vec![Vec3::splat(99.0); 4];
    let bytes = radiance::encode(&buf);
    let stale = StreamEvent::Frame(FrameHeader::for_payload(1, 0, 3, &bytes));
    let outcome = chunk2
        .accumulator
        .lock()
        .unwrap()
        .apply(&stale, Some(&bytes))
        .unwrap();
    assert_eq!(outcome, ApplyOutcome::StaleDropped);
    // ... and one reaching the OLD accumulator touches only that orphaned buffer.
    apply_frame(&chunk1.accumulator, 1, chunk1.first_sample + 3, 2, 7.0);
    assert_eq!(epoch2.remote_done(), 0);
    assert_eq!(epoch2.remote_snapshot(), (vec![Vec3::ZERO; 4], 0));
}

/// Plays one epoch to completion: the remote lane dispatches chunks (the `fail_*`
/// closure decides after how many samples a chunk dies, `None` = completes) while a
/// local tracer claims 2 samples per "frame". Returns every range each side traced
/// and the final (local, remote) counts.
fn play_epoch(
    target: u32,
    local_lane: bool,
    mut fail_after: impl FnMut(u32) -> Option<u32>,
) -> (Vec<(u32, u32)>, u32, u32, Vec<ChunkVerdict>) {
    let epoch = Arc::new(LiveEpoch::new(1, 1, target));
    let mut lane = LiveLane::new(Arc::clone(&epoch), local_lane);
    let mut traced = Vec::new();
    let mut local_count = 0;
    let mut verdicts = Vec::new();
    let mut request_id = 0;
    let mut clock = 0;
    loop {
        request_id += 1;
        clock += 50;
        if let Some(chunk) = lane.next_chunk(request_id, t(clock)) {
            if let Some(done) = fail_after(request_id) {
                let done = done.min(chunk.samples);
                if done > 0 {
                    apply_frame(
                        &chunk.accumulator,
                        request_id,
                        chunk.first_sample,
                        done,
                        1.0,
                    );
                    traced.push((chunk.first_sample, done));
                }
                verdicts.push(lane.chunk_failed(request_id).unwrap());
            } else {
                complete_chunk(&mut lane, &chunk, t(clock + 40));
                traced.push((chunk.first_sample, chunk.samples));
            }
        }
        if local_lane && let Some((start, count)) = epoch.claim_local(2) {
            traced.push((start, count));
            local_count += count;
        }
        if lane.is_finished() && local_count + epoch.remote_done() >= target {
            break;
        }
        assert!(request_id < 10_000, "the epoch never finished");
    }
    assert!(
        epoch.claim_local(2).is_none(),
        "nothing may be left to trace once the count reached the target"
    );
    traced.sort_unstable();
    (traced, local_count, epoch.remote_done(), verdicts)
}

/// A chunk failing midway keeps its prefix, returns the remainder to the local tracer,
/// and the epoch still ends with EXACTLY `target` samples, each index traced once.
#[test]
fn a_failure_mid_chunk_returns_the_remainder_and_the_final_count_is_exact() {
    let target = 64;
    let (traced, local, remote, verdicts) = play_epoch(target, true, |id| (id == 1).then_some(3));
    assert_eq!(verdicts, vec![ChunkVerdict::Continue]);
    assert_eq!(local + remote, target);
    assert_partition(&traced, 0, target);
}

/// Two failures in a row: the lane gives up for this epoch (exactly one `GaveUp`),
/// and local finishes everything that was not traced, count still exact.
#[test]
fn two_consecutive_failures_give_up_and_local_finishes_exactly() {
    let target = 40;
    let (traced, local, remote, verdicts) = play_epoch(target, true, |id| (id <= 2).then_some(1));
    assert_eq!(verdicts, vec![ChunkVerdict::Continue, ChunkVerdict::GaveUp]);
    assert_eq!(remote, 2, "each failed chunk kept its one-sample prefix");
    assert_eq!(local + remote, target);
    assert_partition(&traced, 0, target);
}

/// `RemoteOnly` has no local tracer: a failed chunk's remainder is retried by the lane
/// itself, and the epoch still ends exact.
#[test]
fn remote_only_retries_its_own_remainder() {
    let target = 20;
    let (traced, local, remote, verdicts) = play_epoch(target, false, |id| (id == 1).then_some(5));
    assert_eq!(verdicts, vec![ChunkVerdict::Continue]);
    assert_eq!(local, 0);
    assert_eq!(remote, target);
    assert_partition(&traced, 0, target);
}

// ---- A final-picture (display-only) lane ------------------------------------------

/// A display-only lane asks for the whole budget in ONE request (display frames of
/// separate requests would each show only their own samples), finishes without
/// merging any radiance, and never claims again.
#[test]
fn a_display_only_lane_claims_the_whole_budget_once_and_merges_nothing() {
    let epoch = Arc::new(LiveEpoch::new(2, 2, 256));
    let mut lane = LiveLane::display_only(Arc::clone(&epoch));
    assert!(lane.is_display_only());
    let chunk = lane.next_chunk(7, t(0)).expect("one chunk");
    assert_eq!((chunk.first_sample, chunk.samples), (0, 256));
    // The remote sends display frames only, so the chunk's accumulator stays empty.
    let end = lane.chunk_done(7, t(500)).expect("in flight");
    assert_eq!((end.start, end.count, end.done), (0, 256, 256));
    assert!(lane.is_finished());
    assert_eq!(lane.next_chunk(8, t(600)).map(|c| c.samples), None);
    assert_eq!(epoch.remote_done(), 0, "no radiance was ever merged");
}

/// A failed display-only request is retried whole (there is no local lane to hand
/// its range to), and gives up after the usual number of failures in a row.
#[test]
fn a_failed_display_only_request_is_retried_whole() {
    let epoch = Arc::new(LiveEpoch::new(1, 1, 64));
    let mut lane = LiveLane::display_only(Arc::clone(&epoch));
    let first = lane.next_chunk(1, t(0)).unwrap();
    assert_eq!(
        lane.chunk_failed(first.request_id),
        Some(ChunkVerdict::Continue)
    );
    let retry = lane.next_chunk(2, t(10)).expect("retried");
    assert_eq!((retry.first_sample, retry.samples), (0, 64));
    assert_eq!(
        lane.chunk_failed(retry.request_id),
        Some(ChunkVerdict::GaveUp)
    );
}

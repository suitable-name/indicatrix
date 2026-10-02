//! Unit tests for the export-side liveness deadlines, the retry backoff, the progress
//! span a request's rate is measured from, and `scene_state_from_snapshot`'s
//! pose/bounce-cap handling.

use super::{
    super::rate::remote_request_rate,
    lane::{
        MAX_CONSECUTIVE_REMOTE_FAILURES, REMOTE_RETRY_BACKOFF_INITIAL, REMOTE_RETRY_BACKOFF_MAX,
        pause_remote_lane, remote_retry_backoff,
    },
    run_batch::{
        FIRST_EVENT_TIMEOUT, LIVENESS_TIMEOUT, ProgressSpan, liveness_deadline, transfer_allowance,
    },
    scene_state::scene_state_from_snapshot,
};
use crate::bridge::export_thread::{sample_cursor::SampleCursor, scene_snapshot::SceneSnapshot};
use indicatrix::{geometry::cuts::StandardGemCuts, optics::materials::GemMaterial};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

/// The export's own bounce cap must reach the remote `RenderRequest`, not whatever
/// the live viewport was set to. Uses a value distinct from any plausible viewport
/// default (`RenderContext::default().max_bounces` is 12) to prove a combined
/// local+remote export can't trace the two engines at different caps.
#[test]
fn scene_state_from_snapshot_carries_the_snapshots_own_bounce_cap_not_a_viewport_default() {
    let snapshot = SceneSnapshot {
        yaw: 0.6,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: indicatrix::optics::raytracer::LightingPreset::RingLights,
        max_bounces: 64, // deliberately NOT `RenderContext::default().max_bounces` (12)
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: None,
    };

    let state = scene_state_from_snapshot(&snapshot, 1920, 1080, snapshot.yaw, snapshot.pitch);

    assert_eq!(
        state.max_bounces, 64,
        "the remote RenderRequest's scene must carry the export's OWN bounce cap"
    );
}

/// A caller sweeping the camera across many renders of the SAME static snapshot
/// (the tilt performance video, one frame per swept angle) must see ITS pose reach
/// the remote `SceneState`, not the snapshot's own stored `yaw`/`pitch` -- otherwise
/// a remote-rendered frame would show a different pose than the local CPU/GPU
/// engines traced for the same frame. Uses a pose deliberately different from the
/// snapshot's own to prove the override, not the snapshot, wins.
#[test]
fn scene_state_from_snapshot_uses_the_explicit_pose_override_not_the_snapshots_own_yaw_pitch() {
    let snapshot = SceneSnapshot {
        yaw: 0.6,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: indicatrix::optics::raytracer::LightingPreset::RingLights,
        max_bounces: 12,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: None,
    };

    let state = scene_state_from_snapshot(&snapshot, 1920, 1080, 1.23, -0.45);

    assert_eq!(
        state.yaw, 1.23,
        "the override's yaw must win over the snapshot's own"
    );
    assert_eq!(
        state.pitch, -0.45,
        "the override's pitch must win over the snapshot's own"
    );
    assert_eq!(
        state.distance, snapshot.distance,
        "distance is not swept per frame, so it still comes from the snapshot"
    );
}

// `liveness_deadline`: the pure decision behind the export-side liveness fix --
// see `FIRST_EVENT_TIMEOUT`'s doc comment for the bug this closes.

#[test]
fn liveness_deadline_grants_the_first_event_grace_before_any_update_has_arrived() {
    assert_eq!(
        liveness_deadline(false, 0),
        FIRST_EVENT_TIMEOUT,
        "the wait for a dispatch's very first RemoteUpdate (even Connected) must \
         use the longer grace, not the steady-state deadline"
    );
}

#[test]
fn liveness_deadline_switches_to_the_tighter_steady_state_timeout_once_seen() {
    assert_eq!(
        liveness_deadline(true, 0),
        LIVENESS_TIMEOUT,
        "once a dispatch has produced at least one update, every wait after it must \
         use the steady-state liveness timeout, not the first-event grace"
    );
}

#[test]
fn first_event_timeout_is_strictly_longer_than_liveness_timeout() {
    assert!(FIRST_EVENT_TIMEOUT > LIVENESS_TIMEOUT);
}

/// The 4K regression: one FRAME is ~100 MB, which a ~150 Mbit/s wireless link
/// drains in 5-8 s -- at or past the bare 8 s steady-state deadline. The allowance
/// must lift the deadline well clear of that, and scale with the frame, not a
/// resolution-blind constant.
#[test]
fn liveness_deadline_budgets_a_whole_frame_transfer_on_a_slow_link() {
    let bytes_4k = 3840 * 2160 * indicatrix_net::radiance::BYTES_PER_PIXEL as u64;
    let bytes_1080p = 1920 * 1080 * indicatrix_net::radiance::BYTES_PER_PIXEL as u64;
    let at_4k = liveness_deadline(true, bytes_4k);
    let at_1080p = liveness_deadline(true, bytes_1080p);
    assert!(at_4k >= LIVENESS_TIMEOUT + Duration::from_secs(20));
    assert!(at_4k > at_1080p);
    assert_eq!(transfer_allowance(0), Duration::ZERO);
}

// `ProgressSpan` + `remote_request_rate`: what `run_remote_batch` reports at `DONE`.

#[test]
fn progress_span_has_no_advance_before_two_distinct_reports() {
    let t0 = Instant::now();
    let mut span = ProgressSpan::default();
    assert_eq!(span.advance(), None);
    span.note(t0, 0); // nothing traced yet: not a report of progress
    assert_eq!(span.advance(), None);
    span.note(t0 + Duration::from_secs(1), 100);
    assert_eq!(span.advance(), None, "one report is a point, not a span");
    span.note(t0 + Duration::from_secs(2), 100); // a repeat advances nothing
    assert_eq!(span.advance(), None);
}

#[test]
fn progress_span_measures_from_the_first_to_the_latest_advancing_report() {
    let t0 = Instant::now();
    let mut span = ProgressSpan::default();
    span.note(t0 + Duration::from_secs(1), 100);
    span.note(t0 + Duration::from_secs(3), 300);
    span.note(t0 + Duration::from_secs(4), 300); // a heartbeat: the span must not stretch to it
    span.note(t0 + Duration::from_secs(5), 500);
    span.note(t0 + Duration::from_secs(6), 400); // never moves backwards
    assert_eq!(span.advance(), Some((400, Duration::from_secs(4))));
}

/// A coordinator (or a worker) whose only progress report lands at 20 s of a 25 s
/// request: no span to measure, so the rate is the whole request's, not the window from
/// that report to `DONE` (which would read 1000 / 5 s = 200/s).
#[test]
fn a_single_coarse_progress_jump_yields_the_whole_request_rate() {
    let t0 = Instant::now();
    let mut span = ProgressSpan::default();
    span.note(t0 + Duration::from_secs(20), 1000);
    let whole = Some(40.0);
    for coordinator in [false, true] {
        assert_eq!(
            remote_request_rate(coordinator, 1000, Duration::from_secs(25), span.advance()),
            whole
        );
    }
}

#[test]
fn a_worker_with_two_progress_reports_is_measured_between_them_not_to_done() {
    let t0 = Instant::now();
    let mut span = ProgressSpan::default();
    span.note(t0 + Duration::from_secs(2), 100);
    span.note(t0 + Duration::from_secs(12), 1100);
    // 1000 samples in 10 s of rendering; DONE came 13 s later, after a slow upload.
    assert_eq!(
        remote_request_rate(false, 1100, Duration::from_secs(25), span.advance()),
        Some(100.0)
    );
    // A coordinator is measured over the whole request whatever its progress did.
    assert_eq!(
        remote_request_rate(true, 1100, Duration::from_secs(25), span.advance()),
        Some(44.0)
    );
}

#[test]
fn remote_retry_backoff_doubles_from_the_threshold_and_caps() {
    assert_eq!(
        remote_retry_backoff(MAX_CONSECUTIVE_REMOTE_FAILURES),
        REMOTE_RETRY_BACKOFF_INITIAL
    );
    assert_eq!(
        remote_retry_backoff(MAX_CONSECUTIVE_REMOTE_FAILURES + 1),
        REMOTE_RETRY_BACKOFF_INITIAL * 2
    );
    assert_eq!(remote_retry_backoff(50), REMOTE_RETRY_BACKOFF_MAX);
}

#[test]
fn pause_remote_lane_returns_early_once_the_shared_pool_is_dry() {
    let cursor = SampleCursor::new(0, 4);
    assert_eq!(cursor.claim(4), Some((0, 4)));
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    assert!(!pause_remote_lane(
        &cursor,
        &cancel,
        Duration::from_secs(30)
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
}

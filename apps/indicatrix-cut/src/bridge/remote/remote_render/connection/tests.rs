use super::{super::types::RemoteUpdate, *};

// ---- `route_event`: the stale-response filtering a persistent connection needs --
//
// No socket involved -- `route_event` only ever touches `Option<CurrentRequest>`
// and a decoded `StreamEvent`, so this exercises the actual correctness mechanism
// (`Accumulator::apply`'s epoch check, driven the same way `dispatch_render` drives
// it for a real connection) without a live `indicatrix-worker`.

use glam::Vec3;
use indicatrix_net::{
    client::Accumulator,
    messages::{Done, ErrorMsg, FrameHeader, Progress, Stats},
    radiance,
};

/// Builds a [`CurrentRequest`] whose accumulator has already begun `request_id`'s
/// epoch (mirrors exactly what [`dispatch_render`] does right before sending the
/// request on the wire), plus a shared `Vec` every [`RemoteUpdate`] its `on_update`
/// receives is recorded into, so tests can assert on what was (or wasn't) reported.
fn current_request(
    request_id: u32,
    width: u32,
    height: u32,
) -> (CurrentRequest, Arc<Mutex<Vec<RemoteUpdate>>>) {
    let mut acc = Accumulator::new(width, height);
    acc.begin_request(request_id);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let updates_for_closure = Arc::clone(&updates);
    let on_update = Box::new(move |u: RemoteUpdate| {
        updates_for_closure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(u);
    });
    (
        CurrentRequest {
            request_id,
            accumulator: Arc::new(Mutex::new(acc)),
            on_update,
        },
        updates,
    )
}

#[test]
fn route_event_with_no_current_request_is_a_harmless_no_op() {
    let mut current: Option<CurrentRequest> = None;
    let event = StreamEvent::Progress(Progress {
        request_id: 1,
        samples_done: 5,
    });
    route_event(&mut current, &event, None); // must not panic
    assert!(current.is_none());
}

#[test]
fn route_event_applies_a_matching_frame_and_reports_it() {
    let (cur, updates) = current_request(1, 2, 2);
    let mut current = Some(cur);
    let bytes = radiance::encode(&[Vec3::splat(1.0); 4]);
    let header = FrameHeader::for_payload(1, 0, 3, &bytes);

    route_event(&mut current, &StreamEvent::Frame(header), Some(&bytes));

    assert!(current.is_some(), "a FRAME is never terminal");
    assert!(matches!(
        updates.lock().unwrap()[..],
        [RemoteUpdate::Frame {
            request_id: 1,
            samples_done: 3
        }]
    ));
}

/// The scenario the whole persistent-connection design exists to get right: a
/// reply for a request this connection has already moved on from must never reach
/// the NEW request's accumulator or `on_update` -- simulated here exactly as
/// `dispatch_render` produces it for a real supersede (a fresh `CurrentRequest`
/// whose accumulator's epoch is the NEW id), just without a real socket.
#[test]
fn route_event_drops_a_stale_frame_for_a_superseded_request() {
    let (cur, updates) = current_request(2, 2, 2); // current epoch is 2, not 1
    let mut current = Some(cur);
    let bytes = radiance::encode(&[Vec3::splat(9.0); 4]);
    let stale_header = FrameHeader::for_payload(1, 0, 3, &bytes); // a leftover from id 1

    route_event(
        &mut current,
        &StreamEvent::Frame(stale_header),
        Some(&bytes),
    );

    assert!(
        current.is_some(),
        "the current (id 2) request must be untouched by a stale id-1 reply"
    );
    assert!(
        updates.lock().unwrap().is_empty(),
        "a stale reply for a superseded request must never reach on_update"
    );
}

#[test]
fn route_event_clears_current_on_done() {
    let (cur, updates) = current_request(7, 1, 1);
    let mut current = Some(cur);

    route_event(
        &mut current,
        &StreamEvent::Done(Done {
            request_id: 7,
            cancelled: false,
            stats: Stats {
                samples_done: 10,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
            },
        }),
        None,
    );

    assert!(current.is_none(), "DONE must end this request's epoch");
    assert!(matches!(
        updates.lock().unwrap()[..],
        [RemoteUpdate::Done {
            request_id: 7,
            cancelled: false
        }]
    ));
}

/// A stale `DONE` (for an already-superseded epoch) must be dropped exactly like a
/// stale `FRAME` -- and, since it doesn't match the current epoch, must NOT clear
/// `current`, which is still legitimately in flight for the NEW request.
#[test]
fn route_event_drops_a_stale_done_without_clearing_the_real_current_request() {
    let (cur, updates) = current_request(2, 1, 1);
    let mut current = Some(cur);

    route_event(
        &mut current,
        &StreamEvent::Done(Done {
            request_id: 1, // stale -- current epoch is 2
            cancelled: true,
            stats: Stats {
                samples_done: 1,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
            },
        }),
        None,
    );

    assert!(
        current.is_some(),
        "a stale DONE for a superseded request must never end the CURRENT one"
    );
    assert!(updates.lock().unwrap().is_empty());
}

#[test]
fn route_event_worker_error_clears_current_regardless_of_epoch() {
    let (cur, updates) = current_request(3, 1, 1);
    let mut current = Some(cur);

    route_event(
        &mut current,
        &StreamEvent::Error(ErrorMsg {
            code: 9,
            message: "boom".to_string(),
        }),
        None,
    );

    assert!(current.is_none(), "a WorkerError always ends the request");
    assert!(matches!(
        &updates.lock().unwrap()[..],
        [RemoteUpdate::Failed { request_id: 3, message }] if message == "boom"
    ));
}

// ---- `check_liveness`: the socket-free liveness decision `run_connection`'s `Ok(None)`
// arm applies -- see that function's own doc comment for why this, not a full
// `run_connection` run against a fake socket, is what's unit-tested: `RemoteStream` is a
// concrete `rustls::StreamOwned<ClientConnection, TcpStream>`, with no way to substitute
// an in-memory double the way `bridge::library::client`'s generic `Read + Write` helpers
// allow.

#[test]
fn check_liveness_is_a_no_op_when_nothing_is_current() {
    let mut current: Option<CurrentRequest> = None;
    let long_ago = Instant::now()
        .checked_sub(Duration::from_millis(500))
        .expect("500ms in the past is always representable");
    assert!(!check_liveness(
        &mut current,
        long_ago,
        Duration::from_millis(10)
    ));
    assert!(current.is_none());
}

#[test]
fn check_liveness_is_a_no_op_before_the_deadline() {
    let (cur, updates) = current_request(4, 1, 1);
    let mut current = Some(cur);
    // `last_event` is "now" -- nowhere near the deadline yet.
    assert!(!check_liveness(
        &mut current,
        Instant::now(),
        Duration::from_secs(30)
    ));
    assert!(current.is_some());
    assert!(updates.lock().unwrap().is_empty());
}

/// The scenario [`super::LIVENESS_TIMEOUT`] exists to catch: a worker that accepted a
/// request and then never sends another byte -- `try_read_stream_event` keeps cleanly
/// returning `Ok(None)` ("nothing pending yet", `run_connection`'s own read-timeout
/// tolerance) forever, with nothing else ever telling this side the worker is gone.
/// Simulated here by a `last_event` already further in the past than `timeout` --
/// exactly what `run_connection`'s loop would observe after that many silent polls.
#[test]
fn check_liveness_fails_the_current_request_once_the_deadline_has_passed() {
    let (cur, updates) = current_request(5, 1, 1);
    let mut current = Some(cur);
    let timeout = Duration::from_millis(20);
    // Already well past the deadline.
    let last_event = Instant::now()
        .checked_sub(Duration::from_millis(50))
        .expect("50ms in the past is always representable");

    assert!(check_liveness(&mut current, last_event, timeout));

    assert!(
        current.is_none(),
        "a timed-out request must be dropped so the next dispatch reconnects"
    );
    assert!(matches!(
        &updates.lock().unwrap()[..],
        [RemoteUpdate::Failed { request_id: 5, message }] if message.contains("silent")
    ));
}

/// The complementary case: a worker that keeps emitting events (a `PROGRESS` every
/// tick, say) must never trip the deadline no matter how much WALL-CLOCK time passes,
/// because `run_connection` resets `last_event` on every one -- modelled here by
/// resetting `last_event` to "now" before each check, the same way the real loop's
/// `Ok(Some(..))` arm does right before calling `route_event`.
#[test]
fn check_liveness_never_trips_while_last_event_keeps_being_reset() {
    let (cur, updates) = current_request(6, 1, 1);
    let mut current = Some(cur);
    let timeout = Duration::from_millis(30);

    for _ in 0..5 {
        thread::sleep(Duration::from_millis(10)); // well under `timeout` each time
        let last_event = Instant::now(); // a fresh event "just arrived"
        assert!(!check_liveness(&mut current, last_event, timeout));
    }

    assert!(current.is_some());
    assert!(updates.lock().unwrap().is_empty());
}

// ---- `liveness_deadline`: the pure decision behind the false-positive fix -- which of
// `FIRST_EVENT_TIMEOUT`/`liveness_timeout` applies to the CURRENT idle wait. See that
// function's own doc comment for why the first wait after a dispatch needs more slack:
// confirmed against a real 4K/cadence-20 export where the worker's first tick
// legitimately outran `LIVENESS_TIMEOUT` while genuinely still calculating.

#[test]
fn liveness_deadline_grants_the_first_event_grace_before_any_event_has_arrived() {
    let liveness_timeout = Duration::from_secs(8);
    assert_eq!(
        liveness_deadline(false, liveness_timeout),
        FIRST_EVENT_TIMEOUT,
        "the wait for a request's very first event must use the longer grace, not the \
         steady-state deadline"
    );
}

#[test]
fn liveness_deadline_switches_to_the_tighter_steady_state_timeout_once_seen() {
    let liveness_timeout = Duration::from_millis(123); // an arbitrary, distinctive value
    assert_eq!(
        liveness_deadline(true, liveness_timeout),
        liveness_timeout,
        "once a request has produced at least one event, every wait after it must use \
         the caller's own (heartbeat-derived) liveness_timeout, not the first-event grace"
    );
}

#[test]
fn first_event_timeout_is_strictly_longer_than_liveness_timeout() {
    // The whole point of the two-tier deadline: a worker's first tick (calibration,
    // warm-up, a coarse cadence at a high resolution) is allowed to take longer than
    // every steady-state heartbeat interval after it.
    assert!(FIRST_EVENT_TIMEOUT > LIVENESS_TIMEOUT);
}

// ---- The false-positive scenario itself: a long busy phase this thread spends NOT
// polling the socket, immediately followed by a real event, must never be mistaken for
// silence -- see `check_liveness`'s own doc comment and this module's "Why a busy
// consumer can never make either deadline fire early" doc section for why this holds
// by construction (the deadline is only ever checked right after a just-attempted,
// empty read), pinned here as an explicit regression test rather than left implicit.

#[test]
fn check_liveness_ignores_a_long_busy_phase_that_ends_with_a_fresh_event() {
    let (cur, updates) = current_request(8, 1, 1);
    let mut current = Some(cur);
    let timeout = Duration::from_millis(20);

    // Simulate a long stretch (far past `timeout`) where this thread was doing
    // something else entirely and never touched the socket at all -- `last_event`
    // still reflects whenever the request was last dispatched or last produced a real
    // event, long before this "busy phase" began.
    let stale_last_event = Instant::now()
        .checked_sub(Duration::from_secs(5))
        .expect("5s in the past is always representable");

    // The busy phase ends with the loop resuming reading and immediately getting a
    // REAL event -- exactly `run_connection`'s `Ok(Some(..))` arm, which resets
    // `last_event` to "now" and calls `route_event` BEFORE `check_liveness` could ever
    // be reached for this iteration. `check_liveness` itself is therefore never even
    // called with the stale value in the real loop; this test pins that by showing
    // that even if it WERE (a fresh `last_event` taken right as the event arrives), the
    // stale five-second gap that preceded it has no bearing on the outcome.
    let fresh_last_event = Instant::now();
    assert!(!check_liveness(&mut current, fresh_last_event, timeout));
    assert!(
        current.is_some(),
        "an event arriving right after a long busy phase must never be treated as the \
         worker having gone silent"
    );
    assert!(updates.lock().unwrap().is_empty());

    // For contrast: had the busy phase instead ended with a poll that STILL came back
    // empty, the stale `last_event` from five seconds ago correctly does trip the
    // deadline -- this is genuine, measured silence on the socket, not an artifact of
    // the consumer having been busy.
    assert!(check_liveness(&mut current, stale_last_event, timeout));
    assert!(current.is_none());
}

//! Tests for the liveness/timeout-classification machinery: [`is_stream_timeout`]'s
//! `WriteZero`-counts-too classification, [`poll_for_client_message`]'s tolerance of it,
//! and [`wait_for_tracer_to_stop`]'s cadence-independent heartbeat backstop.
//!
//! `run_stream`'s main loop applies the same gating logic the `wait_for_tracer_to_stop`
//! tests below pin, but that requires a real tracer thread to observe "produces nothing
//! for a while" against. `wait_for_tracer_to_stop` is the same gate with none of that
//! machinery: a plain `Write`, a `SharedState` driven directly, and an injectable
//! interval.

use super::fixtures::{
    decode_events, finish_after, heartbeat_test_request, shared_state, tiny_scene,
};
use crate::stream_emit::{
    TimeoutCache, TimeoutRead,
    emitter::{ClientPoll, poll_for_client_message, wait_for_tracer_to_stop},
    is_stream_timeout,
};
use indicatrix_net::messages::StreamEvent;
use std::{
    io::ErrorKind,
    sync::{Arc, Mutex},
    time::Duration,
};

#[test]
fn is_stream_timeout_recognizes_would_block_timed_out_and_write_zero() {
    for kind in [
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::WriteZero,
    ] {
        assert!(
            is_stream_timeout(&std::io::Error::new(kind, "scripted")),
            "expected {kind:?} to be recognized as a stream timeout"
        );
    }
}

#[test]
fn is_stream_timeout_rejects_unrelated_error_kinds() {
    for kind in [
        ErrorKind::ConnectionReset,
        ErrorKind::UnexpectedEof,
        ErrorKind::Other,
    ] {
        assert!(
            !is_stream_timeout(&std::io::Error::new(kind, "scripted")),
            "expected {kind:?} to NOT be recognized as a stream timeout"
        );
    }
}

/// A `Read + TimeoutRead` double whose very first (and only) read returns one scripted
/// `io::Error`, used to pin `poll_for_client_message`'s classification of that error.
struct ErroringRead(ErrorKind);

impl std::io::Read for ErroringRead {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(self.0, "scripted"))
    }
}

impl TimeoutRead for ErroringRead {
    fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// A real TLS stream can surface a socket timeout as `WriteZero` even from a read call
/// (`rustls::Stream::read`'s `complete_io()` can need to flush outgoing bytes too).
/// Pins that `poll_for_client_message` tolerates it like `WouldBlock`/`TimedOut`, not
/// as a fatal protocol error.
#[test]
fn poll_for_client_message_tolerates_a_write_zero_on_the_first_byte() {
    let mut stream = ErroringRead(ErrorKind::WriteZero);
    let mut timeouts = TimeoutCache::new();
    let result = poll_for_client_message(&mut stream, 1, Duration::from_millis(1), &mut timeouts);
    assert!(matches!(result, Ok(ClientPoll::Pending)));
}

/// A tracer that produces nothing for well over two heartbeat intervals must still see
/// at least two `StreamEvent::Progress` heartbeats -- what a wide-`cadence` cancellation
/// wind-down (or a slow calibration probe, long GPU sub-batch, hybrid CPU-only tail)
/// needs from this function.
#[test]
fn wait_for_tracer_to_stop_heartbeats_at_least_twice_when_the_tracer_produces_nothing() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    let finisher = finish_after(&state, Duration::from_millis(70));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_millis(2), // heartbeat_bound: tiny, so this test stays fast
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    let events = decode_events(&out);
    let progress_count = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Progress(_)))
        .count();
    assert!(
        progress_count >= 2,
        "expected at least 2 heartbeats over > 2 heartbeat intervals, got {progress_count}: {events:?}"
    );
}

/// The counterpart: while `heartbeat_bound` hasn't elapsed yet, no heartbeat goes out
/// at all -- a tick that already proved liveness this recently must suppress the extra
/// write, not double up on it.
#[test]
fn wait_for_tracer_to_stop_suppresses_the_heartbeat_within_the_interval() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    // Finishes well before the bound below could possibly elapse.
    let finisher = finish_after(&state, Duration::from_millis(30));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_secs(10),
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    assert!(
        out.is_empty(),
        "no heartbeat should have been written within the interval: {:?}",
        decode_events(&out)
    );
}

/// The heartbeat's payload is a live read of `samples_done` at the moment it fires --
/// pins that it is never stale.
#[test]
fn wait_for_tracer_to_stop_heartbeat_carries_the_latest_samples_done() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 42)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    let finisher = finish_after(&state, Duration::from_millis(60));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_millis(2),
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    let events = decode_events(&out);
    let last_progress = events.iter().rev().find_map(|e| match e {
        StreamEvent::Progress(p) => Some(p.samples_done),
        _ => None,
    });
    assert_eq!(
        last_progress,
        Some(42),
        "heartbeat must carry the current samples_done: {events:?}"
    );
}

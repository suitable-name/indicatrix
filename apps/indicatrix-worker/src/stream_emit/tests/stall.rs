//! End-to-end tests for the producer-stall watchdog, through [`run_stream_with`]: a
//! producer that never adds a chunk must fail the request with `PRODUCER_STALLED` (and no
//! `DONE`) once `StreamSpec::stall_timeout` passes -- while the emitter's own `PROGRESS`
//! heartbeat keeps going in the meantime, exactly the situation that used to park a
//! batch client on one request forever.

use super::fixtures::{decode_events, render_request_with_preview, tiny_scene};
use crate::stream_emit::{
    Output, ProducerOutcome, ProducerSink, StreamOutcome, StreamSpec, TimeoutRead, TimeoutWrite,
    run_stream_with,
};
use indicatrix_net::messages::{PayloadEncoding, RenderRequest, StreamEvent, error_codes};
use std::{
    io::{ErrorKind, Read, Write},
    time::{Duration, Instant},
};

/// A connection double: nothing to read (a poll times out) until `hang_up_after` has
/// elapsed, then end-of-file (the peer closing); everything written is kept.
struct QuietWire {
    opened: Instant,
    hang_up_after: Duration,
    written: Vec<u8>,
}

impl QuietWire {
    fn new(hang_up_after: Duration) -> Self {
        Self {
            opened: Instant::now(),
            hang_up_after,
            written: Vec::new(),
        }
    }
}

impl Read for QuietWire {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        if self.opened.elapsed() >= self.hang_up_after {
            return Ok(0);
        }
        Err(std::io::Error::new(
            ErrorKind::WouldBlock,
            "nothing pending",
        ))
    }
}

impl Write for QuietWire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for QuietWire {
    fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutWrite for QuietWire {
    fn set_write_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// A producer that never calls `add_chunk`: it only returns once the emitter raises the
/// cancel flag -- a wedged tracer that does eventually notice `cancel`.
fn wedged_producer(sink: ProducerSink) {
    while !sink.is_cancelled() {
        std::thread::sleep(Duration::from_millis(10));
    }
    sink.finish(ProducerOutcome::Cancelled);
}

fn spec(request: &RenderRequest, stall_timeout: Option<Duration>) -> StreamSpec<'_> {
    StreamSpec {
        request,
        payload_encoding: PayloadEncoding::Raw,
        link: None,
        output: Output::Radiance,
        contribution: None,
        stall_timeout,
    }
}

#[test]
fn a_wedged_producer_fails_the_request_with_producer_stalled_and_no_done() {
    let request = render_request_with_preview(tiny_scene());
    let mut wire = QuietWire::new(Duration::from_secs(30));
    let started = Instant::now();

    let (outcome, next) = run_stream_with(
        &mut wire,
        &spec(&request, Some(Duration::from_millis(300))),
        wedged_producer,
    )
    .expect("the stream ends cleanly");
    let elapsed = started.elapsed();

    assert!(next.is_none());
    let StreamOutcome::Failed(error) = outcome else {
        panic!("expected a stall failure, got {outcome:?}");
    };
    assert_eq!(error.code, error_codes::PRODUCER_STALLED);
    assert_eq!(error.request_id, Some(request.request_id));
    let events = decode_events(&wire.written);
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Progress(_))),
        "the emitter heartbeats until it gives up: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Done(_))),
        "a stalled request ends with an ERROR and no DONE: {events:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "the watchdog must not wait on the wedged producer: {elapsed:?}"
    );
}

#[test]
fn without_a_stall_timeout_a_quiet_producer_is_left_alone() {
    let request = render_request_with_preview(tiny_scene());
    // The peer hangs up at 700 ms, which cancels the producer and ends the test; a
    // watchdog wrongly armed at 300 ms would have failed the request long before.
    let mut wire = QuietWire::new(Duration::from_millis(700));
    let started = Instant::now();

    let (outcome, _next) = run_stream_with(&mut wire, &spec(&request, None), wedged_producer)
        .expect("the stream ends cleanly");

    assert_eq!(outcome, StreamOutcome::Completed);
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "the request must have run until the peer hung up"
    );
}

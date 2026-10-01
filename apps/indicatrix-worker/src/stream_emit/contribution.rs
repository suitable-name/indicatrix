//! The viewer's own share of a `FinalImageRequest` (v16): [`ContributionSlot`] is the
//! rendezvous between the connection thread (which reads the `CONTRIBUTION` off the
//! wire, see [`ContributionSlot::receive`]) and the job's producer thread (which waits
//! for it once its own lanes finish, see [`ContributionSlot::await_until`]).
//!
//! # Why a rendezvous, not a channel
//!
//! Both directions need more than "hand over a value": the producer thread must be able
//! to give up after a bounded wait (reclaiming the range itself) while still never
//! reclaiming mid-upload, and the connection thread must know whether a contribution
//! that finishes arriving late is still wanted at all. A `Mutex<SlotState>` plus
//! `Condvar` gives both sides that visibility into the other's progress; a plain
//! one-shot channel would not.

use super::TimeoutCache;
use crate::stream_emit::TimeoutRead;
use glam::Vec3;
use indicatrix_dispatch::SampleRange;
use indicatrix_net::messages::{
    ContributionError, ContributionHeader, ExpectedContribution, NetError,
};
use std::{
    io::Read,
    sync::{Condvar, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// The default wait for a reserved viewer contribution once the server's own lanes
/// finish, when a request doesn't override it -- see [`super::super::coordinator::job::JobConfig::contribution_wait`].
pub const DEFAULT_CONTRIBUTION_WAIT: Duration = Duration::from_secs(30);

/// Per-read timeout while a `CONTRIBUTION` payload streams in -- mirrors
/// `crate::assets::fetch`'s own asset-payload read timeout.
pub const CONTRIBUTION_PAYLOAD_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// What [`ContributionSlot::await_until`] returned.
pub enum Awaited {
    /// The contribution arrived, was valid, and is ready to fold in.
    Arrived(Vec<Vec3>),
    /// No usable contribution: the wait elapsed, or what arrived was invalid. `reason`
    /// is logged by the caller, never sent to the viewer (see this crate's coordinator
    /// job doc comment on why a reclaim is silent).
    Reclaim {
        /// Human-readable reason, for a `tracing::info!` at the call site.
        reason: String,
    },
    /// The request was cancelled while waiting.
    Cancelled,
}

/// The slot's internal state, guarded by [`ContributionSlot::state`].
enum SlotState {
    /// Nothing has arrived yet.
    Waiting,
    /// The connection thread is mid-read of the payload frame -- never reclaimed while
    /// in this state (see [`ContributionSlot::await_until`]).
    Receiving,
    /// A valid contribution, ready to be taken exactly once.
    Arrived(Vec<Vec3>),
    /// What arrived did not decode, or didn't match the expected request/range/size.
    Invalid(String),
    /// [`ContributionSlot::await_until`] gave up (timeout, or an invalid arrival) and
    /// the server is tracing this range itself; a contribution that arrives after this
    /// is dropped (see [`ContributionSlot::receive`]).
    Reclaimed,
    /// [`ContributionSlot::await_until`] already returned [`Awaited::Arrived`]; the
    /// slot is spent.
    Taken,
}

/// The rendezvous for one `FinalImageRequest`'s reserved viewer range -- see the module
/// doc comment.
pub struct ContributionSlot {
    reserved: SampleRange,
    width: u32,
    height: u32,
    state: Mutex<SlotState>,
    changed: Condvar,
}

impl std::fmt::Debug for ContributionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContributionSlot")
            .field("reserved", &self.reserved)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl ContributionSlot {
    /// A fresh slot for `reserved` (the viewer's tail of a `width x height` picture),
    /// nothing arrived yet.
    #[must_use]
    pub const fn new(reserved: SampleRange, width: u32, height: u32) -> Self {
        Self {
            reserved,
            width,
            height,
            state: Mutex::new(SlotState::Waiting),
            changed: Condvar::new(),
        }
    }

    /// The reserved range this slot answers for.
    #[must_use]
    pub const fn reserved(&self) -> SampleRange {
        self.reserved
    }

    /// The connection thread's side: `header` has already been read off the wire (its
    /// `ClientMessage::Contribution` envelope), so this reads and validates the
    /// payload frame that follows, bounded exactly like `messages::contribution`
    /// documents.
    ///
    /// Marks the slot `Receiving` before reading (so [`Self::await_until`] never
    /// reclaims mid-upload), then `Arrived`/`Invalid` once the read finishes -- unless
    /// the slot was already `Reclaimed`/`Taken` by then, in which case the bytes are
    /// simply dropped with an info log (the producer thread has moved on).
    ///
    /// # Errors
    ///
    /// [`NetError`] only for [`ContributionError::TooLarge`]/[`ContributionError::Framing`]
    /// (the connection is out of sync and must be dropped); every other outcome
    /// (including an invalid contribution) is `Ok(())`.
    pub(crate) fn receive<S: Read + TimeoutRead>(
        &self,
        stream: &mut S,
        header: &ContributionHeader,
        request_id: u32,
        timeouts: &mut TimeoutCache,
    ) -> Result<(), NetError> {
        // A slot the producer thread has already moved past (Reclaimed/Taken) stays
        // exactly there -- overwriting it with `Receiving` would erase the very fact
        // `finish_receive` needs to recognise this arrival as late.
        if !self.already_spent() {
            self.set_state(SlotState::Receiving);
        }
        timeouts
            .apply(stream, Some(CONTRIBUTION_PAYLOAD_READ_TIMEOUT))
            .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;

        let expect = ExpectedContribution {
            request_id,
            first_sample: self.reserved.first_sample,
            samples: self.reserved.samples,
            width: self.width,
            height: self.height,
        };
        let mut decoder = indicatrix_net::radiance::PayloadDecoder::new();
        let outcome = indicatrix_net::messages::read_contribution_payload(
            stream,
            header,
            expect,
            &mut decoder,
        );
        self.finish_receive(request_id, outcome)
    }

    /// Sets `state` and wakes [`Self::await_until`].
    fn set_state(&self, state: SlotState) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
        self.changed.notify_all();
    }

    /// The second half of [`Self::receive`]: turns the decode outcome into the slot's
    /// next state (or a hard [`NetError`]), split out so `receive` stays short.
    fn finish_receive(
        &self,
        request_id: u32,
        outcome: Result<Vec<Vec3>, ContributionError>,
    ) -> Result<(), NetError> {
        match outcome {
            Ok(sum) => {
                if self.already_spent() {
                    tracing::info!(
                        request_id,
                        "a CONTRIBUTION arrived after its range was already reclaimed or taken; dropping it"
                    );
                } else {
                    self.set_state(SlotState::Arrived(sum));
                }
                Ok(())
            }
            Err(ContributionError::TooLarge {
                payload_len,
                raw_len,
            }) => Err(NetError::Framing(
                indicatrix_net::framing::FramingError::FrameTooLarge {
                    len: payload_len,
                    max: raw_len,
                },
            )),
            Err(ContributionError::Framing(e)) => Err(NetError::Framing(e)),
            Err(e @ (ContributionError::Mismatch(_) | ContributionError::Radiance(_))) => {
                let reason = e.to_string();
                if self.already_spent() {
                    tracing::info!(
                        request_id,
                        "an invalid CONTRIBUTION arrived after its range was already reclaimed or \
                         taken; dropping it: {reason}"
                    );
                } else {
                    self.set_state(SlotState::Invalid(reason));
                }
                Ok(())
            }
        }
    }

    /// Whether [`Self::await_until`] has already moved past this slot (`Reclaimed`/`Taken`).
    fn already_spent(&self) -> bool {
        matches!(
            *self.state.lock().unwrap_or_else(PoisonError::into_inner),
            SlotState::Reclaimed | SlotState::Taken
        )
    }

    /// The producer thread's side: waits up to `wait` (extended for as long as a
    /// receive is actually in flight) in 50 ms slices for the contribution to arrive,
    /// or `cancelled` to report the request has ended.
    ///
    /// Returns [`Awaited::Arrived`] immediately once the slot holds one (marking it
    /// `Taken`); [`Awaited::Reclaim`] immediately once it holds an invalid one (marking
    /// it `Reclaimed`, no wait), or once `wait` elapses with nothing usable (also
    /// `Reclaimed`); [`Awaited::Cancelled`] as soon as `cancelled()` says so.
    pub fn await_until(&self, wait: Duration, cancelled: impl Fn() -> bool) -> Awaited {
        let mut deadline = Instant::now() + wait;
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match &*guard {
                SlotState::Arrived(_) => {
                    let SlotState::Arrived(sum) = std::mem::replace(&mut *guard, SlotState::Taken)
                    else {
                        unreachable!("just matched SlotState::Arrived");
                    };
                    return Awaited::Arrived(sum);
                }
                SlotState::Invalid(reason) => {
                    let reason = reason.clone();
                    *guard = SlotState::Reclaimed;
                    return Awaited::Reclaim { reason };
                }
                // An upload is in flight: never reclaim mid-read -- keep pushing the
                // deadline out for as long as it continues.
                SlotState::Receiving => deadline = Instant::now() + wait,
                SlotState::Waiting | SlotState::Reclaimed | SlotState::Taken => {}
            }
            if cancelled() {
                return Awaited::Cancelled;
            }
            if Instant::now() >= deadline {
                *guard = SlotState::Reclaimed;
                return Awaited::Reclaim {
                    reason: format!("no contribution within {wait:?}"),
                };
            }
            guard = self
                .changed
                .wait_timeout(guard, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_net::{messages::PayloadEncoding, radiance::PayloadEncoder};
    use std::{
        io::Cursor,
        sync::{Arc, mpsc},
        thread,
    };

    /// A `Read + TimeoutRead` double over an in-memory buffer -- timeouts are a no-op,
    /// exactly enough for these single-threaded/scoped-thread tests.
    struct MockStream(Cursor<Vec<u8>>);

    impl Read for MockStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl TimeoutRead for MockStream {
        fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn contribution_wire(
        request_id: u32,
        range: SampleRange,
        width: u32,
        height: u32,
        value: f32,
    ) -> Vec<u8> {
        let sum = vec![Vec3::splat(value); (width * height) as usize];
        let mut encoder = PayloadEncoder::new(PayloadEncoding::Raw);
        let mut buf = Vec::new();
        indicatrix_net::messages::write_contribution_message(
            &mut buf,
            request_id,
            (range.first_sample, range.samples),
            width,
            height,
            &sum,
            &mut encoder,
        )
        .unwrap();
        buf
    }

    fn header_and_stream(wire: Vec<u8>) -> (ContributionHeader, MockStream) {
        let mut cursor = Cursor::new(wire);
        let msg: indicatrix_net::messages::ClientMessage =
            indicatrix_net::messages::read_message(&mut cursor).unwrap();
        let indicatrix_net::messages::ClientMessage::Contribution(header) = msg else {
            panic!("expected ClientMessage::Contribution");
        };
        (header, MockStream(cursor))
    }

    /// An arrival already sitting in the slot is returned immediately, without waiting.
    #[test]
    fn an_arrival_before_the_wait_is_returned_immediately() {
        let range = SampleRange::new(10, 4);
        let slot = ContributionSlot::new(range, 2, 2);
        let wire = contribution_wire(7, range, 2, 2, 3.0);
        let (header, mut stream) = header_and_stream(wire);
        slot.receive(&mut stream, &header, 7, &mut TimeoutCache::new())
            .unwrap();

        let start = Instant::now();
        let Awaited::Arrived(sum) = slot.await_until(Duration::from_secs(30), || false) else {
            panic!("expected Arrived");
        };
        assert_eq!(sum, vec![Vec3::splat(3.0); 4]);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    /// Nothing ever arrives: the wait elapses and the slot reclaims. A contribution
    /// that shows up afterward is silently dropped (proven by `receive` still
    /// returning `Ok(())` and not panicking on an already-Reclaimed slot).
    #[test]
    fn a_timeout_reclaims_and_a_late_arrival_is_dropped() {
        let range = SampleRange::new(0, 2);
        let slot = ContributionSlot::new(range, 1, 1);
        let outcome = slot.await_until(Duration::from_millis(80), || false);
        assert!(matches!(outcome, Awaited::Reclaim { .. }));

        let wire = contribution_wire(1, range, 1, 1, 9.0);
        let (header, mut stream) = header_and_stream(wire);
        slot.receive(&mut stream, &header, 1, &mut TimeoutCache::new())
            .unwrap();
        // Still reclaimed -- the late arrival must not resurrect it.
        assert!(matches!(*slot.state.lock().unwrap(), SlotState::Reclaimed));
    }

    /// A contribution with the wrong range reclaims immediately, no wait needed.
    #[test]
    fn an_invalid_contribution_reclaims_without_waiting() {
        let range = SampleRange::new(0, 2);
        let slot = ContributionSlot::new(range, 1, 1);
        // Built for a different range than the slot expects.
        let wire = contribution_wire(1, SampleRange::new(5, 2), 1, 1, 1.0);
        let (header, mut stream) = header_and_stream(wire);
        slot.receive(&mut stream, &header, 1, &mut TimeoutCache::new())
            .unwrap();

        let start = Instant::now();
        let outcome = slot.await_until(Duration::from_secs(30), || false);
        assert!(matches!(outcome, Awaited::Reclaim { .. }));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    /// While a receive is genuinely in flight, the wait must not expire out from under
    /// it -- proven with a short configured wait that a slower (but still eventually
    /// successful) receive still lands as `Arrived`.
    ///
    /// The waiter reports every pass of its wait loop through `cancelled()`, so the slot
    /// is only completed after the waiter has provably looped well past its 60 ms wait
    /// (one pass per 50 ms slice; without the extension it returns after about three),
    /// however late the thread was scheduled.
    #[test]
    fn a_receiving_upload_extends_the_deadline() {
        /// Wait passes that must be seen: at 50 ms per pass, far beyond the 60 ms wait.
        const PASSES: usize = 6;
        let range = SampleRange::new(0, 2);
        let slot = Arc::new(ContributionSlot::new(range, 1, 1));
        // Mark `Receiving` up front, exactly like `receive` does before it blocks on
        // I/O, and hold it there past the short wait below.
        slot.set_state(SlotState::Receiving);

        let (pass_tx, pass_rx) = mpsc::channel();
        let waiter = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                slot.await_until(Duration::from_millis(60), || {
                    let _ = pass_tx.send(());
                    false
                })
            })
        };
        for pass in 1..=PASSES {
            pass_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| {
                    panic!(
                        "the waiter stopped after {} of {PASSES} wait passes: its 60 ms wait \
                         expired while an upload was still being received",
                        pass - 1
                    )
                });
        }
        slot.set_state(SlotState::Arrived(vec![Vec3::ONE]));
        let outcome = waiter.join().unwrap();
        assert!(matches!(outcome, Awaited::Arrived(_)));
    }

    /// `cancelled()` ends the wait right away, even with time left.
    #[test]
    fn cancel_ends_the_wait() {
        let range = SampleRange::new(0, 2);
        let slot = ContributionSlot::new(range, 1, 1);
        let start = Instant::now();
        let outcome = slot.await_until(Duration::from_secs(30), || true);
        assert!(matches!(outcome, Awaited::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}

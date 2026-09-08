//! [`Accumulator`]: the epoch-gated radiance sum a viewer keeps for one connection's
//! worth of remote rendering.
//!
//! # The invariant this exists to enforce
//!
//! A `CANCEL` can be in flight past a worker that's already mid-batch, so
//! `FRAME`/`PREVIEW`/`DONE` payloads for a just-cancelled request may still arrive after
//! the client has moved on to a new `request_id`. The rule that makes this safe is
//! mechanical and applies uniformly to every message: **honor/sum/display a payload iff
//! its `request_id` matches the accumulator's CURRENT epoch; drop everything else.**
//! [`Accumulator::apply`] is the one place a `request_id` is ever compared; every other
//! piece of client code goes through it rather than re-implementing the check.
//!
//! # `FRAME` vs `PREVIEW`, mirrored from `crate::messages`
//!
//! `FRAME` is a full-resolution DELTA: [`Accumulator::apply`] sums it straight into
//! [`Accumulator::buffer`]. `PREVIEW` is a cumulative, reduced-resolution snapshot: it
//! REPLACES [`Accumulator::last_preview`] rather than being summed into anything --
//! summing a reduced-resolution buffer into a full-resolution one isn't even
//! dimensionally sound. See `crate::messages`'s own module docs for the full argument.

use crate::{
    messages::{Done, ErrorMsg, PreviewHeader, Progress, StreamEvent},
    radiance,
};
use glam::Vec3;

/// A `PREVIEW`'s payload, decoded and kept as the accumulator's single
/// "most recent preview" slot -- see the module doc comment on why a new one replaces
/// (never sums with) whatever was there before.
#[derive(Debug, Clone, PartialEq)]
// `buffer: Vec<Vec3>` is float data, so `Eq` isn't derivable here.
pub struct PreviewSnapshot {
    pub width: u32,
    pub height: u32,
    pub samples_done: u32,
    pub buffer: Vec<Vec3>,
}

/// What [`Accumulator::apply`] did with one [`StreamEvent`].
///
/// `Copy`: every variant only carries `u32`/`bool` tags, so callers can take it by
/// value cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// A `FRAME` delta was summed into [`Accumulator::buffer`].
    FrameSummed { samples_done: u32 },
    /// A `PREVIEW` snapshot replaced [`Accumulator::last_preview`].
    PreviewReplaced,
    /// A `PROGRESS` ping for the current epoch.
    Progress { samples_done: u32 },
    /// `DONE` for the current epoch -- the request finished or was cancelled; no
    /// further payload for this epoch follows.
    Done { cancelled: bool },
    /// `ERROR` -- the worker rejected the request. Not epoch-gated: an error carries no
    /// `request_id` of its own, so it's always surfaced to the caller.
    WorkerError,
    /// The event's `request_id` didn't match [`Accumulator::current_request_id`] --
    /// dropped without touching `buffer` or `last_preview`. See the module doc comment.
    StaleDropped,
}

/// The epoch-gated radiance sum for one session's remote render.
///
/// Owns a `width * height` buffer sized once at construction (the render resolution is
/// session-wide, not per-request -- see the crate's `client` module docs), zeroed
/// every time [`begin_request`](Self::begin_request) starts a new epoch.
pub struct Accumulator {
    width: u32,
    height: u32,
    /// `None` before the first [`begin_request`](Self::begin_request) call -- nothing
    /// is ever "current epoch" yet, so every event is dropped as stale until a request
    /// actually starts.
    current_request_id: Option<u32>,
    buffer: Vec<Vec3>,
    samples_done: u32,
    last_preview: Option<PreviewSnapshot>,
}

impl Accumulator {
    /// Builds an empty accumulator for a `width * height` render. No request is
    /// current yet -- see [`begin_request`](Self::begin_request).
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let pixel_count = width as usize * height as usize;
        Self {
            width,
            height,
            current_request_id: None,
            buffer: vec![Vec3::ZERO; pixel_count],
            samples_done: 0,
            last_preview: None,
        }
    }

    /// Starts a new epoch: `request_id` becomes [`current_request_id`](Self::current_request_id),
    /// `buffer` is zeroed, `samples_done` resets to 0, and any pending
    /// [`last_preview`](Self::last_preview) is cleared -- a preview from the previous
    /// (now superseded) request is exactly as stale as a `FRAME` from it would be.
    ///
    /// Call this the moment a new `RenderRequest` is sent, NOT when its reply starts
    /// arriving: any bytes for the OLD epoch still in flight on the wire must see the
    /// new epoch already in place so [`apply`](Self::apply) drops them.
    pub fn begin_request(&mut self, request_id: u32) {
        self.current_request_id = Some(request_id);
        self.buffer.fill(Vec3::ZERO);
        self.samples_done = 0;
        self.last_preview = None;
    }

    #[must_use]
    pub const fn current_request_id(&self) -> Option<u32> {
        self.current_request_id
    }

    #[must_use]
    pub fn buffer(&self) -> &[Vec3] {
        &self.buffer
    }

    #[must_use]
    pub const fn samples_done(&self) -> u32 {
        self.samples_done
    }

    #[must_use]
    pub const fn last_preview(&self) -> Option<&PreviewSnapshot> {
        self.last_preview.as_ref()
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Applies one [`StreamEvent`] (as read by [`crate::messages::read_stream_event`],
    /// `payload` being whatever it returned alongside), enforcing the epoch rule
    /// described in the module doc comment.
    ///
    /// [`StreamEvent::Error`] is never epoch-gated (it carries no `request_id`) and is
    /// always reported as [`ApplyOutcome::WorkerError`] regardless of what's currently
    /// current.
    ///
    /// # Errors
    ///
    /// Returns [`crate::radiance::RadianceError`] if a `FRAME`/`PREVIEW` payload fails
    /// to decode against its own header's declared dimensions -- this is a malformed
    /// payload, not a stale one (a stale payload is still well-formed; it's just for
    /// the wrong epoch), so it's a hard error rather than a silent drop.
    pub fn apply(
        &mut self,
        event: &StreamEvent,
        payload: Option<&[u8]>,
    ) -> Result<ApplyOutcome, radiance::RadianceError> {
        match event {
            StreamEvent::Frame(header) => {
                if Some(header.request_id) != self.current_request_id {
                    return Ok(ApplyOutcome::StaleDropped);
                }
                let bytes = payload.unwrap_or(&[]);
                // `decode_borrowed`, not `decode`: this only ever sums the decoded
                // delta straight into `self.buffer` below and never keeps it around,
                // so there's no reason to pay for an owned `Vec<Vec3>` copy of a
                // potentially full-frame payload first.
                let delta = radiance::decode_borrowed(bytes, self.width, self.height)?;
                for (acc, d) in self.buffer.iter_mut().zip(delta) {
                    *acc += *d;
                }
                self.samples_done = self.samples_done.saturating_add(header.samples);
                Ok(ApplyOutcome::FrameSummed {
                    samples_done: self.samples_done,
                })
            }
            StreamEvent::Preview(header) => {
                if Some(header.request_id) != self.current_request_id {
                    return Ok(ApplyOutcome::StaleDropped);
                }
                let bytes = payload.unwrap_or(&[]);
                let buffer = radiance::decode(bytes, header.width, header.height)?;
                self.last_preview = Some(PreviewSnapshot {
                    width: header.width,
                    height: header.height,
                    samples_done: header.samples_done,
                    buffer,
                });
                Ok(ApplyOutcome::PreviewReplaced)
            }
            StreamEvent::Progress(Progress {
                request_id,
                samples_done,
            }) => {
                if Some(*request_id) != self.current_request_id {
                    return Ok(ApplyOutcome::StaleDropped);
                }
                Ok(ApplyOutcome::Progress {
                    samples_done: *samples_done,
                })
            }
            StreamEvent::Done(Done {
                request_id,
                cancelled,
                ..
            }) => {
                if Some(*request_id) != self.current_request_id {
                    return Ok(ApplyOutcome::StaleDropped);
                }
                Ok(ApplyOutcome::Done {
                    cancelled: *cancelled,
                })
            }
            StreamEvent::Error(ErrorMsg { .. }) => Ok(ApplyOutcome::WorkerError),
        }
    }
}

/// Decodes a `PREVIEW` header's declared dimensions.
///
/// Exposed purely so callers that only want to know a preview's shape (without
/// decoding its payload) don't have to reach into [`PreviewHeader`] themselves; used by
/// `apps/indicatrix-cut`'s bridge layer when sizing a display buffer ahead of the first
/// preview.
#[must_use]
pub const fn preview_dimensions(header: &PreviewHeader) -> (u32, u32) {
    (header.width, header.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{FrameHeader, PreviewHeader, Stats};

    fn frame_event(request_id: u32, value: f32, pixel_count: usize) -> (StreamEvent, Vec<u8>) {
        let buf = vec![Vec3::splat(value); pixel_count];
        let bytes = radiance::encode(&buf);
        let header = FrameHeader::for_payload(request_id, 0, 1, &bytes);
        (StreamEvent::Frame(header), bytes)
    }

    fn preview_event(
        request_id: u32,
        value: f32,
        width: u32,
        height: u32,
        samples_done: u32,
    ) -> (StreamEvent, Vec<u8>) {
        let buf = vec![Vec3::splat(value); (width * height) as usize];
        let bytes = radiance::encode(&buf);
        let header = PreviewHeader::for_payload(request_id, width, height, samples_done, &bytes);
        (StreamEvent::Preview(header), bytes)
    }

    #[test]
    fn events_before_any_begin_request_are_all_dropped_as_stale() {
        let mut acc = Accumulator::new(2, 2);
        let (event, bytes) = frame_event(1, 1.0, 4);
        let outcome = acc.apply(&event, Some(&bytes)).unwrap();
        assert_eq!(outcome, ApplyOutcome::StaleDropped);
        assert!(acc.buffer().iter().all(|v| *v == Vec3::ZERO));
    }

    #[test]
    fn frame_deltas_for_the_current_epoch_sum_into_the_buffer() {
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(1);

        let (event_a, bytes_a) = frame_event(1, 1.0, 4);
        acc.apply(&event_a, Some(&bytes_a)).unwrap();
        let (event_b, bytes_b) = frame_event(1, 2.0, 4);
        acc.apply(&event_b, Some(&bytes_b)).unwrap();

        for v in acc.buffer() {
            assert!((*v - Vec3::splat(3.0)).length() < 1e-6);
        }
    }

    #[test]
    fn preview_snapshots_replace_rather_than_sum_and_never_touch_the_frame_buffer() {
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(1);

        let (frame, frame_bytes) = frame_event(1, 5.0, 4);
        acc.apply(&frame, Some(&frame_bytes)).unwrap();

        let (preview_a, bytes_a) = preview_event(1, 10.0, 1, 1, 4);
        let outcome = acc.apply(&preview_a, Some(&bytes_a)).unwrap();
        assert_eq!(outcome, ApplyOutcome::PreviewReplaced);
        assert_eq!(acc.last_preview().unwrap().buffer, vec![Vec3::splat(10.0)]);

        let (preview_b, bytes_b) = preview_event(1, 20.0, 1, 1, 8);
        acc.apply(&preview_b, Some(&bytes_b)).unwrap();
        // The SECOND preview replaced the first -- not summed with it (20.0, not 30.0).
        assert_eq!(acc.last_preview().unwrap().buffer, vec![Vec3::splat(20.0)]);
        assert_eq!(acc.last_preview().unwrap().samples_done, 8);

        // The frame buffer (a completely separate slot) is untouched by either preview.
        for v in acc.buffer() {
            assert!((*v - Vec3::splat(5.0)).length() < 1e-6);
        }
    }

    /// The invariant the whole module exists for: a delta for a just-superseded epoch,
    /// still in flight when the next request begins, must never merge into the new
    /// request's accumulation.
    #[test]
    fn a_frame_for_a_superseded_epoch_is_dropped_not_summed_into_the_new_one() {
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(1);
        let (frame1, bytes1) = frame_event(1, 100.0, 4);
        acc.apply(&frame1, Some(&bytes1)).unwrap();

        // The client moves on to a new request before this in-flight FRAME is read.
        acc.begin_request(2);
        assert_eq!(acc.current_request_id(), Some(2));
        assert!(
            acc.buffer().iter().all(|v| *v == Vec3::ZERO),
            "begin_request must zero the buffer"
        );

        // The stale epoch-1 frame arrives (or is finally processed) here.
        let stale_outcome = acc.apply(&frame1, Some(&bytes1)).unwrap();
        assert_eq!(stale_outcome, ApplyOutcome::StaleDropped);
        assert!(
            acc.buffer().iter().all(|v| *v == Vec3::ZERO),
            "a stale epoch-1 delta must never be summed into epoch 2's buffer"
        );

        // A legitimate epoch-2 frame DOES sum normally.
        let (frame2, bytes2) = frame_event(2, 7.0, 4);
        acc.apply(&frame2, Some(&bytes2)).unwrap();
        for v in acc.buffer() {
            assert!((*v - Vec3::splat(7.0)).length() < 1e-6);
        }
    }

    #[test]
    fn a_preview_for_a_superseded_epoch_is_dropped_too() {
        let mut acc = Accumulator::new(4, 4);
        acc.begin_request(1);
        let (preview1, bytes1) = preview_event(1, 42.0, 2, 2, 4);
        acc.apply(&preview1, Some(&bytes1)).unwrap();
        assert!(acc.last_preview().is_some());

        acc.begin_request(2);
        assert!(
            acc.last_preview().is_none(),
            "begin_request must clear the preview slot"
        );

        let stale_outcome = acc.apply(&preview1, Some(&bytes1)).unwrap();
        assert_eq!(stale_outcome, ApplyOutcome::StaleDropped);
        assert!(acc.last_preview().is_none());
    }

    #[test]
    fn progress_and_done_are_also_epoch_gated() {
        let mut acc = Accumulator::new(1, 1);
        acc.begin_request(5);

        let stale_progress = StreamEvent::Progress(Progress {
            request_id: 4,
            samples_done: 99,
        });
        assert_eq!(
            acc.apply(&stale_progress, None).unwrap(),
            ApplyOutcome::StaleDropped
        );

        let current_progress = StreamEvent::Progress(Progress {
            request_id: 5,
            samples_done: 12,
        });
        assert_eq!(
            acc.apply(&current_progress, None).unwrap(),
            ApplyOutcome::Progress { samples_done: 12 }
        );

        let stale_done = StreamEvent::Done(Done {
            request_id: 4,
            cancelled: true,
            stats: Stats {
                samples_done: 1,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
            },
        });
        assert_eq!(
            acc.apply(&stale_done, None).unwrap(),
            ApplyOutcome::StaleDropped
        );

        let current_done = StreamEvent::Done(Done {
            request_id: 5,
            cancelled: false,
            stats: Stats {
                samples_done: 12,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
            },
        });
        assert_eq!(
            acc.apply(&current_done, None).unwrap(),
            ApplyOutcome::Done { cancelled: false }
        );
    }

    #[test]
    fn worker_error_is_reported_regardless_of_current_epoch() {
        let mut acc = Accumulator::new(1, 1);
        // No begin_request call at all -- current epoch is None.
        let event = StreamEvent::Error(ErrorMsg {
            code: 2,
            message: "validation failed".to_string(),
        });
        assert_eq!(acc.apply(&event, None).unwrap(), ApplyOutcome::WorkerError);
    }

    #[test]
    fn begin_request_resets_samples_done() {
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(1);
        let (frame, bytes) = frame_event(1, 1.0, 4);
        acc.apply(&frame, Some(&bytes)).unwrap();
        assert!(acc.samples_done() > 0);

        acc.begin_request(2);
        assert_eq!(acc.samples_done(), 0);
    }

    #[test]
    fn a_malformed_frame_payload_is_a_hard_error_not_a_stale_drop() {
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(1);
        let header = FrameHeader {
            request_id: 1,
            first_sample: 0,
            samples: 1,
            payload_len: 3,
        };
        let bad_bytes = vec![0u8; 3]; // not a multiple of BYTES_PER_PIXEL, and wrong length for 2x2
        let event = StreamEvent::Frame(header);
        assert!(acc.apply(&event, Some(&bad_bytes)).is_err());
    }
}

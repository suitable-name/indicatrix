//! [`RemoteProgress`]: the cross-thread state a running remote lane publishes for
//! `run_export`'s progress-reporting closure to read while the lane is still running.

use glam::Vec3;
use indicatrix_net::client::Accumulator;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

/// Cross-thread state [`super::lane::run_remote_lane`] publishes for `run_export`'s
/// progress- reporting closure to read WHILE the lane is still running -- a lane runs
/// many sequential chunk dispatches, each needing its OWN fresh `Accumulator` (see
/// [`super::lane::run_remote_lane`] for why one can't be reused across chunks).
///
/// Every field is behind `Mutex`/`AtomicU32` interior mutability so this can be shared
/// as a plain `&RemoteProgress` between the remote lane's thread and the thread
/// driving local -- no `Arc` needed, since both threads live only for the duration of
/// the `thread::scope` call in `run_export` that this outlives.
pub(in crate::bridge::export_thread) struct RemoteProgress {
    /// The sum of every chunk this lane has FULLY merged so far -- like `gpu_accum`,
    /// only ever folded into the export's own `accum` ONCE, after the concurrent phase
    /// has completely ended (merging earlier would race local's CPU threads, which
    /// write to overlapping pixel indices).
    pub(super) accum: Mutex<Vec<Vec3>>,
    /// How many samples are folded into `accum` above. Kept alongside it so progress
    /// reporting is one atomic load, not a buffer scan every tick.
    pub(super) traced: AtomicU32,
    /// The chunk currently in flight, if any -- `Some` only while a
    /// `super::run_batch::run_remote_batch` call is in progress. Lets the progress
    /// closure show live sub-chunk progress rather than one that only advances in
    /// ~22-second jumps.
    pub(super) in_flight: Mutex<Option<Arc<Mutex<Accumulator>>>>,
    /// User-facing notes this lane produced mid-export -- queued since more than one
    /// can happen over a long export. `run_export`'s progress closure drains at most
    /// one per tick via [`take_note`](Self::take_note).
    pub(super) notes: Mutex<VecDeque<String>>,
}

impl RemoteProgress {
    pub(in crate::bridge::export_thread) fn new(pixel_count: usize) -> Self {
        Self {
            accum: Mutex::new(vec![Vec3::ZERO; pixel_count]),
            traced: AtomicU32::new(0),
            in_flight: Mutex::new(None),
            notes: Mutex::new(VecDeque::new()),
        }
    }

    /// Total samples this lane has traced so far: every fully-merged chunk, plus
    /// whatever the in-flight chunk has already streamed back. Never double-counts a
    /// chunk that finishes between the two reads below -- at worst slightly
    /// UNDER-reports for one tick, never over-, since `run_remote_lane` always clears
    /// `in_flight` before adding to `traced`.
    pub(in crate::bridge::export_thread) fn samples_done(&self) -> u32 {
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let live = in_flight.as_ref().map_or(0, |acc| {
            acc.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .samples_done()
        });
        drop(in_flight);
        self.traced.load(Ordering::Relaxed) + live
    }

    /// A live combined preview buffer: every fully-merged chunk's radiance plus the
    /// in-flight chunk's current buffer, pixel-summed. Always returns a full
    /// `width * height` buffer -- an export with no remote contribution yet is simply
    /// all zero, exactly like `gpu_accum` before the GPU's first batch lands.
    pub(in crate::bridge::export_thread) fn preview_buffer(&self) -> Vec<Vec3> {
        let mut buf = self
            .accum
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(acc) = in_flight.as_ref() {
            let acc = acc.lock().unwrap_or_else(PoisonError::into_inner);
            for (dst, src) in buf.iter_mut().zip(acc.buffer()) {
                *dst += *src;
            }
        }
        drop(in_flight);
        buf
    }

    /// Drains the OLDEST queued note, if any. `run_export`'s progress closure calls
    /// this once per tick so a mid-export chunk failure surfaces as a toast.
    pub(in crate::bridge::export_thread) fn take_note(&self) -> Option<String> {
        self.notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Consumes `self` and returns everything this lane ever fully merged, for
    /// `run_export` to fold into its own `accum` exactly once after the concurrent
    /// phase has ended -- mirrors `gpu_accum`'s single end-of-export merge.
    pub(in crate::bridge::export_thread) fn into_buffer(self) -> Vec<Vec3> {
        self.accum
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

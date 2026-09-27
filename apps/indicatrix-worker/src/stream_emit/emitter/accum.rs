//! Delta coalescing and the emitter's own accumulation state: [`PendingDelta`] is the
//! shared buffer un-emitted `FRAME` deltas sum into between emissions, and
//! [`EmitterAccum`] is the running total [`super::run_stream`]'s emitter folds each
//! taken delta into.

use super::{
    super::tracer::SharedState,
    display::{DisplayDenoiser, DisplayUpdate},
};
use glam::Vec3;
use indicatrix_net::{
    SceneState,
    messages::PayloadEncoding,
    radiance::{self, EncodedPayload, PayloadEncoder},
};
use std::sync::Mutex;

/// A full-resolution radiance delta accumulated (coalesced) since it was last taken --
/// the shared buffer `run_tracer`'s sub-batches fold into and `run_stream`'s emitter
/// drains on the cadence.
///
/// Coalescing two adjacent deltas is exactly `PendingDelta::add` twice before one
/// `take`: contributions sum elementwise and sample ranges merge into one contiguous
/// union, so a delta sent as two separate `FRAME`s or one coalesced one sums identically.
pub(in crate::stream_emit) struct PendingDelta {
    buffer: Vec<Vec3>,
    /// `(first_sample, samples)` of the buffer's contents so far, or `None` if nothing
    /// has been folded in since the last `take`.
    range: Option<(u32, u32)>,
}

impl PendingDelta {
    pub(in crate::stream_emit) fn new(pixel_count: usize) -> Self {
        Self {
            buffer: vec![Vec3::ZERO; pixel_count],
            range: None,
        }
    }

    /// Folds `contribution` (the sum for sample sub-range `[first_sample, first_sample +
    /// samples)`) into this delta. Must immediately follow whatever range is already
    /// pending; debug-asserted since a violation would mean `run_tracer` is broken, not
    /// caller-supplied data.
    pub(in crate::stream_emit) fn add(
        &mut self,
        first_sample: u32,
        samples: u32,
        contribution: &[Vec3],
    ) {
        debug_assert_eq!(contribution.len(), self.buffer.len());
        match self.range {
            None => {
                self.buffer.copy_from_slice(contribution);
                self.range = Some((first_sample, samples));
            }
            Some((range_first, range_samples)) => {
                debug_assert_eq!(
                    first_sample,
                    range_first + range_samples,
                    "sub-batches must coalesce in contiguous sample order"
                );
                for (acc, c) in self.buffer.iter_mut().zip(contribution) {
                    *acc += *c;
                }
                self.range = Some((range_first, range_samples + samples));
            }
        }
    }

    /// Folds `contribution`, the sum of `samples` samples that need NOT be contiguous
    /// with what is already pending -- a coordinator's chunk from any lane. The pending
    /// range then reads `(anchor, total)`: a SET of `total` samples inside the request
    /// that starts at `anchor`, which is exactly what a coordinator `FRAME` header says
    /// (`first_sample = request.first_sample`, `samples` exact).
    pub(in crate::stream_emit) fn add_set(
        &mut self,
        anchor: u32,
        samples: u32,
        contribution: &[Vec3],
    ) {
        debug_assert_eq!(contribution.len(), self.buffer.len());
        match self.range {
            None => {
                self.buffer.copy_from_slice(contribution);
                self.range = Some((anchor, samples));
            }
            Some((range_first, range_samples)) => {
                for (acc, c) in self.buffer.iter_mut().zip(contribution) {
                    *acc += *c;
                }
                self.range = Some((range_first, range_samples + samples));
            }
        }
    }

    /// Exchanges this delta's buffer with `spare` via `mem::swap` -- O(1), no allocation
    /// or per-pixel copy -- resetting this delta to empty and leaving what was pending
    /// in `spare` for the caller. Returns the previous range (`None` if nothing had been
    /// added since the last swap).
    ///
    /// `spare` must be the same length as this delta's buffer (debug-asserted); its
    /// contents on the way in are never inspected, since [`Self::add`]'s first call
    /// after this always overwrites the whole buffer via `copy_from_slice`.
    ///
    /// The O(1) alternative to a take-then-clone: [`EmitterAccum`] is the one caller,
    /// and this is what keeps `emit_tick`'s only work under the shared `Mutex` to the
    /// swap plus reading `samples_done`.
    pub(in crate::stream_emit) fn swap_with(
        &mut self,
        spare: &mut Vec<Vec3>,
    ) -> Option<(u32, u32)> {
        debug_assert_eq!(
            spare.len(),
            self.buffer.len(),
            "swap_with's spare buffer must match this delta's pixel count"
        );
        std::mem::swap(&mut self.buffer, spare);
        self.range.take()
    }
}

/// The emitter's own accumulation state: a running total maintained by folding in every
/// delta taken from [`SharedState::pending_delta`], plus the spare buffer that delta
/// gets swapped into. Owned solely by `run_stream`'s emitter -- never behind the shared
/// `Mutex`, never touched by the tracer thread -- so building a `PREVIEW` or the
/// `FinalOnly` final `FRAME` never needs the lock and can never stall `run_tracer`.
///
/// [`Self::swap_and_fold`] is called every cadence tick and once more in `emit_final`,
/// regardless of `TransferMode`: `running_total` must stay current for
/// `PREVIEW` under every transfer mode, and for `FinalOnly`'s own final `FRAME` at the
/// end. Whether the swapped-out delta is ALSO written to the wire as a `FRAME` is a
/// separate decision `emit_tick`/`emit_final` make afterward.
pub(in crate::stream_emit) struct EmitterAccum {
    /// The full-resolution cumulative sum of every delta folded in so far. Read (never
    /// written) by [`super::emit::write_preview`] and by `emit_final`'s
    /// `TransferMode::FinalOnly` branch.
    running_total: Vec<Vec3>,
    /// Exchanged with [`SharedState::pending_delta`]'s buffer via
    /// [`PendingDelta::swap_with`] on every [`Self::swap_and_fold`] call; holds whatever
    /// delta was most recently swapped out until the next swap overwrites it.
    spare: Vec<Vec3>,
    /// The connection's negotiated payload encoding (v14), with its reusable scratch
    /// buffers and compression context -- one per request, owned by the emitter thread,
    /// so compression never runs under the shared `Mutex`.
    encoder: PayloadEncoder,
    /// A `DisplayOnly` request's denoise thread, started by the first
    /// [`Self::display_tick`] (never for any other transfer mode).
    display: Option<DisplayDenoiser>,
}

impl EmitterAccum {
    /// Test-only: an accumulator whose payloads go out raw (the pre-v14 behaviour).
    #[cfg(test)]
    pub(in crate::stream_emit) fn new(pixel_count: usize) -> Self {
        Self::with_encoding(pixel_count, PayloadEncoding::Raw)
    }

    /// An accumulator whose `FRAME`/`PREVIEW` payloads are encoded with `encoding`
    /// (the connection's negotiated one).
    pub(in crate::stream_emit) fn with_encoding(
        pixel_count: usize,
        encoding: PayloadEncoding,
    ) -> Self {
        Self {
            running_total: vec![Vec3::ZERO; pixel_count],
            spare: vec![Vec3::ZERO; pixel_count],
            encoder: PayloadEncoder::new(encoding),
            display: None,
        }
    }

    /// Test-only: seeds `running_total` directly, standing in for "several ticks'
    /// worth of deltas have already been folded in", without needing to actually drive
    /// a `SharedState` through that many swaps first.
    #[cfg(test)]
    pub(in crate::stream_emit) fn from_running_total(running_total: Vec<Vec3>) -> Self {
        let spare = vec![Vec3::ZERO; running_total.len()];
        Self {
            running_total,
            spare,
            encoder: PayloadEncoder::new(PayloadEncoding::Raw),
            display: None,
        }
    }

    /// The delta most recently swapped out by [`Self::swap_and_fold`], encoded for the
    /// wire (zero-copy when the encoding is `Raw`) -- meaningful only when that call
    /// returned `Some`; otherwise stale-but-harmless (already folded into
    /// `running_total`), since no caller reads this without first checking the range.
    pub(in crate::stream_emit) fn encoded_delta(&mut self) -> EncodedPayload<'_> {
        self.encoder.encode(radiance::as_bytes(&self.spare))
    }

    /// [`Self::running_total`], encoded for the wire -- the `FinalOnly` final `FRAME`.
    pub(in crate::stream_emit) fn encoded_running_total(&mut self) -> EncodedPayload<'_> {
        self.encoder.encode(radiance::as_bytes(&self.running_total))
    }

    /// The connection's negotiated payload encoding (what a `DISPLAY_FRAME`'s
    /// `DisplayEncoding` is derived from).
    pub(in crate::stream_emit) const fn encoding(&self) -> PayloadEncoding {
        self.encoder.encoding()
    }

    /// Any other radiance buffer (a downsampled `PREVIEW`), encoded with this request's
    /// encoder.
    pub(in crate::stream_emit) fn encode_other<'a>(
        &'a mut self,
        buffer: &'a [Vec3],
    ) -> EncodedPayload<'a> {
        self.encoder.encode(radiance::as_bytes(buffer))
    }

    /// Locks `state` just long enough to swap [`SharedState::pending_delta`]'s buffer
    /// into `self.spare` and read `samples_done`, then releases the lock before doing
    /// anything else -- no clone or socket write ever happens while this `Mutex` is
    /// held. Once unlocked, if a delta was swapped out, folds it into
    /// `self.running_total` elementwise -- the only place `running_total` is updated,
    /// entirely outside the lock.
    ///
    /// Returns the swapped-out delta's `(first_sample, samples)` range alongside
    /// `samples_done`, read under the same lock acquisition so the two can never
    /// disagree about which tick they describe.
    pub(in crate::stream_emit) fn swap_and_fold(
        &mut self,
        state: &Mutex<SharedState>,
    ) -> (Option<(u32, u32)>, u32) {
        let (range, samples_done) = {
            let mut guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let range = guard.pending_delta.swap_with(&mut self.spare);
            (range, guard.samples_done)
        };
        if range.is_some() {
            for (total, c) in self.running_total.iter_mut().zip(self.spare.iter()) {
                *total += *c;
            }
        }
        (range, samples_done)
    }

    /// The full-resolution cumulative sum of every delta folded in so far -- this
    /// emitter's own copy, never read from `SharedState`.
    pub(in crate::stream_emit) fn running_total(&self) -> &[Vec3] {
        &self.running_total
    }

    /// A `DisplayOnly` cadence tick's picture work (see `super::display`): starts this
    /// request's denoise thread for `scene` on first use, then
    /// [`DisplayDenoiser::tick`]s it with the running total (`fresh`: samples were folded
    /// in by this tick's swap).
    pub(in crate::stream_emit) fn display_tick(
        &mut self,
        scene: &SceneState,
        fresh: bool,
        samples_done: u32,
    ) -> DisplayUpdate {
        self.display
            .get_or_insert_with(|| DisplayDenoiser::spawn(scene))
            .tick(fresh, samples_done, &self.running_total)
    }

    /// Takes this request's display denoiser (for the final picture), if a tick started
    /// one.
    pub(in crate::stream_emit) const fn take_display(&mut self) -> Option<DisplayDenoiser> {
        self.display.take()
    }
}

//! The caller-facing half of one batch run ([`BatchOutput`]), split out of `backend.rs` to
//! keep that file within its size budget. A child module of `backend` so it reaches the
//! backend's private scratch pool.

use glam::Vec3;

use super::{GpuAccumulate, GpuBackend, GpuBatchItem};
use crate::renderer::gpu::frame::BatchEvent;

/// The caller-facing half of one batch run: the output buffers, the per-picture scratch
/// buffers and the `on_done` sink, with the bookkeeping that every picture is reported
/// exactly once and in input order.
pub(super) struct BatchOutput<'b, 'a> {
    pub(super) items: &'b mut [GpuBatchItem<'a>],
    /// One scratch per picture: empty until the picture starts, taken back when it is
    /// reported.
    pub(super) scratches: Vec<Vec<Vec3>>,
    pub(super) on_done: &'b mut dyn FnMut(usize, GpuAccumulate),
    /// Number of pictures already reported (the next one to report has this index).
    pub(super) reported: usize,
}

impl BatchOutput<'_, '_> {
    /// Reports picture `index`: on `Done` its scratch is added into the caller's buffer
    /// first (the single path's final `+=`), then the scratch goes back to the pool.
    pub(super) fn report(&mut self, backend: &GpuBackend, index: usize, outcome: GpuAccumulate) {
        debug_assert_eq!(
            index, self.reported,
            "batch outcomes must be reported in input order"
        );
        let scratch = std::mem::take(&mut self.scratches[index]);
        if outcome == GpuAccumulate::Done {
            for (total, traced) in self.items[index].out.iter_mut().zip(&scratch) {
                *total += *traced;
            }
        }
        backend.return_scratch(scratch);
        self.reported = index + 1;
        (self.on_done)(index, outcome);
    }

    /// Reports what the renderer's stepper concluded.
    pub(super) fn deliver(&mut self, backend: &GpuBackend, event: BatchEvent) {
        match event {
            BatchEvent::Done(index) => self.report(backend, index, GpuAccumulate::Done),
            BatchEvent::Declined(index) => self.report(backend, index, GpuAccumulate::Declined),
            BatchEvent::Cancelled(first) => {
                for index in first..self.items.len() {
                    self.report(backend, index, GpuAccumulate::Cancelled);
                }
            }
        }
    }

    /// Reports every picture not reported yet with `outcome`.
    pub(super) fn finish_rest(&mut self, backend: &GpuBackend, outcome: GpuAccumulate) {
        for index in self.reported..self.items.len() {
            self.report(backend, index, outcome);
        }
    }
}

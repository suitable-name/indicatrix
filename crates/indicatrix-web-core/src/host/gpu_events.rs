//! How the render pool reacts to what the GPU Worker sends: adapter and compile events for
//! the page's admission gate, and the answers to full passes.
//!
//! Every answer to a full pass is matched against the [`crate::gpu_ledger::GpuLedger`]: a
//! live answer is merged or its pass handed back to the planner, a stale one (a replaced
//! scene, the self-check) only frees the Worker's slot. No outcome waits for the silence
//! watchdog.

use std::rc::Rc;

use super::{
    gpu_slot::GpuSlot,
    now_ms,
    render_callbacks::{Deferred, GpuEvent},
    render_pool::{Inner, SlotState},
};
use crate::{
    gpu_ledger::Answer,
    protocol::{FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole},
    render::{Accumulator, ChunkOutcome},
};

/// The GPU Worker's handler is installed: sends it `Init`. A post that fails ends the slot
/// and tells the page's admission gate the GPU is unavailable.
fn init_gpu_worker(slot: &mut GpuSlot, deferred: &mut Deferred) {
    slot.state = SlotState::Initializing;
    let init = ToWorker::Init {
        protocol_version: PROTOCOL_VERSION,
        role: WorkerRole::Gpu,
        worker_index: 0,
    };
    if let Err(error) = slot.handle.post(&init) {
        slot.state = SlotState::Failed;
        deferred.gpu_events.push(GpuEvent::Unavailable {
            reason: error.clone(),
        });
        deferred.errors.push(error);
    }
}

/// The GPU Worker reported `message` as an error. Before the adapter was reported this ends
/// the slot, since the Worker never became ready.
fn gpu_worker_error(slot: &mut GpuSlot, message: String, deferred: &mut Deferred) {
    if slot.state != SlotState::Ready {
        slot.state = SlotState::Failed;
        deferred.gpu_events.push(GpuEvent::Unavailable {
            reason: message.clone(),
        });
    }
    deferred.errors.push(message);
}

impl Inner {
    /// Handles one reply from the GPU render Worker (spawn `spawn_id`).
    pub(super) fn on_gpu_message(&mut self, spawn_id: u64, message: FromWorker) -> Deferred {
        let Some(slot) = self.gpu.as_mut().filter(|s| s.spawn_id == spawn_id) else {
            return Deferred::default();
        };
        slot.last_heard_ms = now_ms();
        let mut deferred = Deferred::default();
        match message {
            FromWorker::Loaded { .. } => init_gpu_worker(slot, &mut deferred),
            FromWorker::GpuAdapter {
                vendor,
                architecture,
                device,
                description,
                backend,
                max_storage_buffers_per_stage,
                max_storage_binding,
                max_buffer,
            } => {
                slot.state = SlotState::Ready;
                slot.max_storage_binding = max_storage_binding;
                self.send_scene_to_gpu(&mut deferred, true);
                deferred.gpu_events.push(GpuEvent::Adapter {
                    vendor,
                    architecture,
                    device,
                    description,
                    backend,
                    max_storage_buffers_per_stage,
                    max_storage_binding,
                    max_buffer,
                });
                deferred.absorb(self.schedule());
            }
            FromWorker::GpuUnavailable { reason } => {
                slot.state = SlotState::Failed;
                deferred.gpu_events.push(GpuEvent::Unavailable { reason });
            }
            FromWorker::GpuCompiled { kernel, class, ms } => {
                deferred
                    .gpu_events
                    .push(GpuEvent::Compiled { kernel, class, ms });
            }
            FromWorker::GpuCompileFailed {
                kernel,
                class,
                reason,
                ms,
            } => {
                deferred.gpu_events.push(GpuEvent::CompileFailed {
                    kernel,
                    class,
                    reason,
                    ms,
                });
            }
            FromWorker::FullPassResult {
                scene_id,
                pass_id,
                sample_offset,
                spp,
                sums,
                elapsed_ms,
            } => self.gpu_pass_result(
                (scene_id, pass_id, sample_offset, spp),
                sums,
                elapsed_ms,
                &mut deferred,
            ),
            FromWorker::ChunkDropped {
                scene_id,
                sample_offset,
                ..
            } => self.gpu_pass_dropped(scene_id, sample_offset, &mut deferred),
            FromWorker::SceneError { scene_id, message } => {
                if scene_id == self.scene_id {
                    slot.scene_rejected = Some(scene_id);
                }
                deferred
                    .gpu_events
                    .push(GpuEvent::SceneRejected { scene_id, message });
            }
            FromWorker::GpuLost { reason } => {
                self.fail_gpu_slot(&reason, &mut deferred);
                deferred.errors.push(format!("GPU device lost: {reason}"));
                deferred.absorb(self.schedule());
            }
            FromWorker::Error { message } => gpu_worker_error(slot, message, &mut deferred),
            _ => {}
        }
        deferred
    }

    /// The Worker answered a full pass with its pixels.
    fn gpu_pass_result(
        &mut self,
        (scene_id, pass_id, sample_offset, spp): (u64, u64, u32, u32),
        sums: Vec<[f32; 3]>,
        elapsed_ms: f64,
        deferred: &mut Deferred,
    ) {
        let Some(slot) = self.gpu.as_mut() else {
            return;
        };
        let answer = slot.ledger.answered(scene_id, Some(pass_id), sample_offset);
        // The pass is merged only when it is finite and non-negative; "any pixel lit" is
        // asked of the self-check alone (a dark scene is a valid scene).
        let finite = Accumulator::full_pass_sums_valid(&sums);
        let sums_valid = finite && sums.iter().any(|s| s[0] > 0.0 || s[1] > 0.0 || s[2] > 0.0);
        deferred.gpu_events.push(GpuEvent::FullPassResult {
            scene_id,
            pass_id,
            sample_offset,
            spp,
            sums_valid,
            elapsed_ms,
        });
        let Answer::Live(_) = answer else {
            // A replaced scene's pass or the self-check: only the slot is freed.
            deferred.absorb(self.schedule());
            return;
        };
        slot.resent_scene = None;
        if !finite {
            self.planner.return_gpu_pass(pass_id);
            self.fail_gpu_slot("GPU returned an invalid image", deferred);
            deferred.absorb(self.schedule());
            return;
        }
        let (width, height) = self
            .spec
            .as_ref()
            .map_or((0, 0), |spec| (spec.width, spec.height));
        let total_samples = u64::from(width) * u64::from(height) * u64::from(spp);
        self.planner.record_gpu_rate(total_samples, elapsed_ms);
        let outcome = self.accumulator.borrow_mut().add_full_pass_owned(
            scene_id,
            pass_id,
            sample_offset,
            spp,
            sums,
        );
        match outcome {
            ChunkOutcome::Committed { .. } => {
                deferred.progress = Some(Rc::clone(&self.accumulator));
            }
            ChunkOutcome::Rejected(rejection) => {
                self.planner.return_gpu_pass(pass_id);
                self.fail_gpu_slot(&format!("GPU pass refused: {rejection:?}"), deferred);
                deferred
                    .errors
                    .push(format!("a GPU full pass was refused: {rejection:?}"));
            }
            ChunkOutcome::Pending | ChunkOutcome::Stale => {}
        }
        deferred.absorb(self.schedule());
    }

    /// The Worker answered a full pass with "no such scene" (or, after an `Error`, with a
    /// pass it would not trace). A live pass goes back to the planner at once. The first
    /// time per scene the Worker is given the scene again before the next pass; a second
    /// loss, or a scene it could not build, stops GPU tracing.
    fn gpu_pass_dropped(&mut self, scene_id: u64, sample_offset: u32, deferred: &mut Deferred) {
        let Some(slot) = self.gpu.as_mut() else {
            return;
        };
        deferred.gpu_events.push(GpuEvent::PassDropped { scene_id });
        if let Answer::Live(pass) = slot.ledger.answered(scene_id, None, sample_offset) {
            self.planner.return_gpu_pass(pass.pass_id);
            if slot.scene_rejected == Some(scene_id) {
                // The scene itself is unbuildable; the CPU Workers report it.
            } else if slot.resent_scene == Some(scene_id) {
                self.fail_gpu_slot("GPU worker lost the scene", deferred);
            } else {
                slot.resent_scene = Some(scene_id);
                slot.sent_scene_id = None;
            }
        }
        deferred.absorb(self.schedule());
    }

    /// The GPU render Worker reported an error event or crashed.
    pub(super) fn on_gpu_crash(&mut self, spawn_id: u64, message: &str) -> Deferred {
        let mut deferred = Deferred::default();
        if self.gpu.as_ref().is_some_and(|s| s.spawn_id == spawn_id) {
            self.fail_gpu_slot(message, &mut deferred);
            deferred.errors.push(format!("GPU worker error: {message}"));
            deferred.absorb(self.schedule());
        }
        deferred
    }
}

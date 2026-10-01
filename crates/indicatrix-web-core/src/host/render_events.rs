//! How the render pool reacts to what its Workers send.
//!
//! Chunk results, lost scenes, crashes, silence and the picture Worker's replies: the
//! `Inner` methods the Workers' `onmessage` and `error` closures (and the watchdog) call.
//! The scheduling side lives in [`super::render_pool`].

use std::rc::Rc;

use glam::Vec3;

use super::{
    now_ms,
    render_callbacks::Deferred,
    render_pool::{InFlight, Inner, MAX_AUTO_RESTARTS, SlotState},
    watchdog::silence_limit_ms,
};
use crate::{
    protocol::{FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole},
    render::{ChunkOutcome, partition_len},
};

impl Inner {
    /// Handles one reply from Worker `index` (spawn `spawn_id`).
    pub(super) fn on_message(
        &mut self,
        index: usize,
        spawn_id: u64,
        message: FromWorker,
    ) -> Deferred {
        let Some(slot) = self.slots.get_mut(index).filter(|s| s.spawn_id == spawn_id) else {
            return Deferred::default();
        };
        slot.last_heard_ms = now_ms();
        let mut deferred = Deferred::default();
        match message {
            FromWorker::Loaded { .. } => {
                slot.state = SlotState::Initializing;
                let init = ToWorker::Init {
                    protocol_version: PROTOCOL_VERSION,
                    role: WorkerRole::Render,
                    worker_index: index as u32,
                };
                if let Err(error) = slot.handle.post(&init) {
                    slot.state = SlotState::Failed;
                    deferred.errors.push(error);
                }
            }
            FromWorker::Ready { .. } => {
                slot.state = SlotState::Ready;
                self.send_state_to(index, &mut deferred);
                deferred.absorb(self.schedule());
            }
            message @ FromWorker::ChunkResult { .. } => {
                slot.busy = false;
                slot.chunk = None;
                slot.crashes = 0;
                self.chunk_result(message, &mut deferred);
            }
            FromWorker::ChunkDropped { scene_id, .. } => {
                slot.busy = false;
                let lost = slot.chunk.take();
                self.chunk_dropped(index, scene_id, lost, &mut deferred);
            }
            FromWorker::ChunkAborted { scene_id, .. } => {
                slot.busy = false;
                slot.chunk = None;
                // The pool only stops chunks of scenes it has replaced.
                if scene_id == self.scene_id {
                    self.scene_failed = true;
                    deferred
                        .errors
                        .push("a render chunk of the current scene was stopped".to_string());
                }
                deferred.absorb(self.schedule());
            }
            FromWorker::SceneError { scene_id, message } => {
                if scene_id == self.scene_id {
                    self.scene_failed = true;
                    deferred.errors.push(message);
                }
            }
            FromWorker::HdrError { message, .. } => deferred.errors.push(message),
            FromWorker::Error { message } => {
                if slot.state != SlotState::Ready {
                    slot.state = SlotState::Failed;
                }
                deferred.errors.push(message);
            }
            // Pictures and solves are answered by other Workers.
            FromWorker::HdrLoaded { .. }
            | FromWorker::Progress { .. }
            | FromWorker::SolveResult { .. }
            | FromWorker::Picture { .. }
            | FromWorker::PictureFailed { .. } => {}
        }
        deferred
    }

    /// Merges a `ChunkResult` of the current scene (others are dropped) and schedules
    /// the Worker's next chunk.
    fn chunk_result(&mut self, message: FromWorker, deferred: &mut Deferred) {
        if let FromWorker::ChunkResult {
            scene_id,
            first_pixel,
            stride,
            sample_offset,
            spp,
            sums,
            elapsed_ms,
        } = message
            && scene_id == self.scene_id
        {
            self.planner.record_timing(first_pixel, spp, elapsed_ms);
            let sums: Vec<Vec3> = sums.into_iter().map(Vec3::from_array).collect();
            let outcome = self.accumulator.borrow_mut().add_chunk(
                scene_id,
                first_pixel,
                stride,
                sample_offset,
                spp,
                sums,
            );
            match outcome {
                ChunkOutcome::Committed { .. } => {
                    deferred.progress = Some(Rc::clone(&self.accumulator));
                }
                ChunkOutcome::Rejected(rejection) => deferred
                    .errors
                    .push(format!("a render chunk was refused: {rejection:?}")),
                ChunkOutcome::Pending | ChunkOutcome::Stale => {}
            }
        }
        deferred.absorb(self.schedule());
    }

    /// A Worker answered a chunk with "no such scene". Once per Worker and scene the pool
    /// gives the Worker its scene again and re-plans the chunk; a second loss, or a scene
    /// whose own error already said why, stops the scene.
    fn chunk_dropped(
        &mut self,
        index: usize,
        scene_id: u64,
        lost: Option<InFlight>,
        deferred: &mut Deferred,
    ) {
        let may_resend = self
            .slots
            .get(index)
            .is_some_and(|slot| slot.resent_scene != Some(scene_id));
        if scene_id != self.scene_id || self.scene_failed {
            // A chunk of a replaced scene, or of one whose error was already reported.
        } else if let Some(lost) = lost.filter(|_| may_resend) {
            self.slots[index].resent_scene = Some(scene_id);
            self.planner.unassign(&lost.assignment);
            self.send_state_to(index, deferred);
        } else {
            self.scene_failed = true;
            deferred.errors.push(format!(
                "render worker {index} lost the scene it was tracing and could not be given it again"
            ));
        }
        deferred.absorb(self.schedule());
    }

    /// A Worker crashed (its `error` event) or went silent: it is replaced, up to
    /// [`MAX_AUTO_RESTARTS`] times without a delivered chunk, and the scene restarts
    /// without the chunk it lost. After that it stays failed for the user to restart.
    fn crash_slot(&mut self, index: usize, message: &str) -> Deferred {
        let mut deferred = Deferred::default();
        let Some(slot) = self.slots.get_mut(index) else {
            return deferred;
        };
        slot.crashes += 1;
        slot.busy = false;
        slot.chunk = None;
        if slot.crashes <= MAX_AUTO_RESTARTS {
            match self.replace_worker(index) {
                Ok(()) => {
                    deferred.errors.push(format!(
                        "A render worker stopped ({message}) and was restarted."
                    ));
                    deferred.absorb(self.restart_scene());
                    return deferred;
                }
                Err(error) => deferred.errors.push(error),
            }
        }
        if let Some(slot) = self.slots.get_mut(index) {
            slot.state = SlotState::Failed;
        }
        self.scene_failed = true;
        deferred.errors.push(message.to_string());
        deferred
    }

    /// Worker `index` (spawn `spawn_id`) reported an error event.
    pub(super) fn on_crash(&mut self, index: usize, spawn_id: u64, message: &str) -> Deferred {
        if self
            .slots
            .get(index)
            .is_some_and(|s| s.spawn_id == spawn_id)
        {
            self.crash_slot(index, message)
        } else {
            Deferred::default()
        }
    }

    /// The watchdog's look: Workers holding a chunk for longer than
    /// [`silence_limit_ms`] allows are treated as crashed.
    pub(super) fn check_silence(&mut self, now: f64) -> Deferred {
        let frame_pixels = self
            .spec
            .as_ref()
            .map_or(0, |spec| spec.width.saturating_mul(spec.height));
        let hung: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.busy && slot.state == SlotState::Ready)
            .filter(|(_, slot)| {
                slot.chunk.as_ref().is_some_and(|chunk| {
                    let own = partition_len(
                        frame_pixels,
                        chunk.assignment.first_pixel,
                        chunk.assignment.stride,
                    );
                    now - slot.last_heard_ms > silence_limit_ms(own, chunk.assignment.spp)
                })
            })
            .map(|(index, _)| index)
            .collect();
        let mut deferred = Deferred::default();
        for index in hung {
            deferred.absorb(self.crash_slot(index, "it stopped answering"));
        }
        deferred
    }

    /// Sends a newly ready Worker the HDR map and scene it missed.
    fn send_state_to(&self, index: usize, deferred: &mut Deferred) {
        let slot = &self.slots[index];
        if let Some((id, bytes)) = &self.hdr {
            let message = ToWorker::HdrMap {
                id: *id,
                bytes: bytes.as_ref().clone(),
            };
            if let Err(error) = slot.handle.post(&message) {
                deferred.errors.push(error);
            }
        }
        if let Some(spec) = &self.spec {
            let message = ToWorker::SetScene {
                scene_id: self.scene_id,
                spec: spec.clone(),
            };
            if let Err(error) = slot.handle.post(&message) {
                deferred.errors.push(error);
            }
        }
    }

    /// Handles one reply from the picture Worker (spawn `spawn_id`).
    pub(super) fn on_picture_message(&mut self, spawn_id: u64, message: FromWorker) -> Deferred {
        let Some(worker) = self
            .picture_worker
            .as_mut()
            .filter(|w| w.spawn_id == spawn_id)
        else {
            return Deferred::default();
        };
        let deferred = worker.on_message(message);
        if worker.broken {
            self.drop_picture_worker();
        }
        deferred
    }

    /// The picture Worker crashed: every picture it owed fails, and the next request
    /// starts a new one.
    pub(super) fn on_picture_crash(&mut self, spawn_id: u64, message: &str) -> Deferred {
        let Some(worker) = self
            .picture_worker
            .as_mut()
            .filter(|w| w.spawn_id == spawn_id)
        else {
            return Deferred::default();
        };
        let deferred = worker.on_crash(message);
        self.drop_picture_worker();
        deferred
    }
}

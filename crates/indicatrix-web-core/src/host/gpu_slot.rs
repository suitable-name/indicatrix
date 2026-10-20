//! The GPU render Worker's slot in the pool: its state, the public GPU calls of
//! [`RenderPool`], and the scheduling of full passes onto it.
//!
//! Rules this file keeps (plan sections 6.4-6.6):
//! - The slot is free only when the Worker has answered everything it was sent
//!   ([`GpuLedger::is_busy`]); a scene restart never frees it early.
//! - The Worker holds ONE scene. After the self-check replaced it, or after it lost it, the
//!   real scene (and the HDR map) is posted again before the next pass.
//! - A pass the slot gives up on (GPU turned off, Worker terminated, lost, silent) goes
//!   back to the planner, so CPU partitions trace it.

use super::{
    handle::WorkerHandle,
    now_ms,
    render_callbacks::{Deferred, GpuEvent},
    render_pool::{Inner, RenderPool, SlotState},
    render_spawn::spawn_gpu_worker,
};
use crate::{
    gpu_admission,
    gpu_ledger::{GpuLedger, LivePass},
    protocol::ToWorker,
    scene::SceneSpec,
};

pub(super) struct GpuSlot {
    pub(super) handle: WorkerHandle,
    pub(super) spawn_id: u64,
    pub(super) state: SlotState,
    /// The passes posted and not yet answered.
    pub(super) ledger: GpuLedger,
    pub(super) last_heard_ms: f64,
    pub(super) max_storage_binding: u64,
    /// The id of the real scene the Worker was last given (`None`: it holds none, or the
    /// self-check's).
    pub(super) sent_scene_id: Option<u64>,
    /// The scene the Worker was already given a second time after it lost it.
    pub(super) resent_scene: Option<u64>,
    /// The scene the Worker could not build; no pass is posted for it.
    pub(super) scene_rejected: Option<u64>,
}

/// What an eligibility verdict was computed for (the scene id names the tools too: every
/// `set_scene_with_tools` is a new scene).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VerdictKey {
    scene_id: u64,
    hdr_texels: Option<u64>,
    max_storage_binding: u64,
}

impl RenderPool {
    /// Turns the GPU render worker on or off. Turning it off hands a pass in flight back
    /// to the planner and terminates the Worker.
    pub fn set_gpu_enabled(&self, enabled: bool) {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.gpu_enabled = enabled;
            if enabled {
                inner.ensure_gpu_worker()
            } else {
                inner.drop_gpu_worker()
            }
        };
        self.finish(deferred);
    }

    /// Whether GPU rendering is enabled in settings.
    #[must_use]
    pub fn is_gpu_enabled(&self) -> bool {
        self.inner.borrow().gpu_enabled
    }

    /// Sets whether GPU rendering is admitted for execution.
    pub fn set_gpu_admitted(&self, admitted: bool) {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.gpu_admitted = admitted;
            inner.schedule()
        };
        self.finish(deferred);
    }

    /// Whether GPU rendering is admitted for execution.
    #[must_use]
    pub fn is_gpu_admitted(&self) -> bool {
        self.inner.borrow().gpu_admitted
    }

    /// Whether the GPU worker is ready to receive commands.
    #[must_use]
    pub fn is_gpu_ready(&self) -> bool {
        self.inner
            .borrow()
            .gpu
            .as_ref()
            .is_some_and(|g| g.state == SlotState::Ready)
    }

    /// The maximum storage buffer binding size reported by the GPU adapter, or 0.
    #[must_use]
    pub fn gpu_max_storage_binding(&self) -> u64 {
        self.inner
            .borrow()
            .gpu
            .as_ref()
            .map_or(0, |g| g.max_storage_binding)
    }

    /// Registers a callback for GPU events.
    pub fn on_gpu_event(&self, callback: impl FnMut(GpuEvent) + 'static) {
        *self.callbacks.gpu_event.borrow_mut() = Some(Box::new(callback));
    }

    /// Sends a compilation request to the GPU worker.
    ///
    /// # Errors
    ///
    /// When there is no GPU worker or the post fails.
    pub fn send_gpu_compile(
        &self,
        class: u32,
        kernels: Vec<String>,
        soft_ms: u32,
        hard_ms: u32,
    ) -> Result<(), String> {
        let mut inner = self.inner.borrow_mut();
        let slot = inner.gpu.as_mut().ok_or("GPU worker is not running")?;
        slot.last_heard_ms = now_ms();
        slot.handle.post(&ToWorker::GpuCompile {
            class,
            kernels,
            soft_ms,
            hard_ms,
        })
    }

    /// Runs the self-check: gives the GPU worker `scene` (under `scene_id`) and one pass of
    /// it. The Worker then holds the self-check scene instead of the real one; the pool
    /// posts the real scene (and HDR map) again before the next real pass, and the answer
    /// is reported as [`GpuEvent::FullPassResult`] / [`GpuEvent::PassDropped`] /
    /// [`GpuEvent::SceneRejected`] with `scene_id`.
    ///
    /// # Errors
    ///
    /// When the GPU worker is not ready or a post fails.
    pub fn run_gpu_self_check(
        &self,
        scene_id: u64,
        scene: SceneSpec,
        spp: u32,
    ) -> Result<(), String> {
        let mut inner = self.inner.borrow_mut();
        let slot = inner
            .gpu
            .as_mut()
            .filter(|g| g.state == SlotState::Ready)
            .ok_or("GPU worker is not ready")?;
        slot.handle.post(&ToWorker::SetScene {
            scene_id,
            spec: scene,
        })?;
        slot.handle.post(&ToWorker::TraceFullPass {
            scene_id,
            pass_id: 0,
            sample_offset: 0,
            spp,
        })?;
        slot.sent_scene_id = None;
        slot.ledger.begin_foreign();
        slot.last_heard_ms = now_ms();
        Ok(())
    }

    /// Terminates the GPU worker and drops its slot; a pass in flight goes back to the
    /// planner.
    pub fn terminate_gpu(&self) {
        let deferred = self.inner.borrow_mut().drop_gpu_worker();
        self.finish(deferred);
    }

    /// Returns why the current scene is ineligible for GPU tracing, or None if eligible.
    #[must_use]
    pub fn gpu_ineligibility_reason(&self) -> Option<&'static str> {
        let inner = self.inner.borrow();
        let spec = inner.spec.as_ref()?;
        let max_binding = inner.gpu.as_ref().map_or(0, |g| g.max_storage_binding);
        gpu_admission::engine_ineligibility_reason(
            inner.engine,
            spec,
            inner.tools.len(),
            max_binding,
            inner.hdr_texels,
            false,
        )
    }
}

impl Inner {
    /// Starts the GPU Worker unless a working one exists.
    pub(super) fn ensure_gpu_worker(&mut self) -> Deferred {
        if self.gpu.is_some() {
            return Deferred::default();
        }
        let spawn_id = self.next_spawn_id;
        self.next_spawn_id += 1;
        match spawn_gpu_worker(&self.self_weak, &self.callbacks, spawn_id) {
            Ok(handle) => {
                self.gpu = Some(GpuSlot {
                    handle,
                    spawn_id,
                    state: SlotState::Booting,
                    ledger: GpuLedger::default(),
                    last_heard_ms: now_ms(),
                    max_storage_binding: 0,
                    sent_scene_id: None,
                    resent_scene: None,
                    scene_rejected: None,
                });
                Deferred::default()
            }
            Err(reason) => {
                let mut deferred = Deferred::default();
                deferred.gpu_events.push(GpuEvent::Unavailable { reason });
                deferred
            }
        }
    }

    /// Terminates the GPU Worker and drops its slot. The pass it was tracing goes back to
    /// the planner, so the CPU partitions trace it.
    pub(super) fn drop_gpu_worker(&mut self) -> Deferred {
        if let Some(mut gpu) = self.gpu.take() {
            if let Some(pass) = gpu.ledger.clear() {
                self.planner.return_gpu_pass(pass.pass_id);
            }
            gpu.handle.terminate();
            self.retired.push(gpu.handle);
        }
        self.schedule()
    }

    /// The GPU slot is given up on: Failed, its live pass back to the planner, and a
    /// [`GpuEvent::Lost`] for the page.
    pub(super) fn fail_gpu_slot(&mut self, reason: &str, deferred: &mut Deferred) {
        if let Some(slot) = self.gpu.as_mut() {
            slot.state = SlotState::Failed;
            if let Some(pass) = slot.ledger.clear() {
                self.planner.return_gpu_pass(pass.pass_id);
            }
        }
        deferred.gpu_events.push(GpuEvent::Lost {
            reason: reason.to_string(),
        });
    }

    /// Posts the current scene (and, with `with_hdr`, the HDR map it names) to a ready GPU
    /// Worker and remembers that it holds it. A map is large, so it is posted again only
    /// when the Worker may have lost it.
    pub(super) fn send_scene_to_gpu(&mut self, deferred: &mut Deferred, with_hdr: bool) {
        let (Some(slot), Some(spec)) = (self.gpu.as_mut(), self.spec.as_ref()) else {
            return;
        };
        if slot.state != SlotState::Ready {
            return;
        }
        let posted = if with_hdr && let Some((id, bytes)) = &self.hdr {
            slot.handle.post(&ToWorker::HdrMap {
                id: *id,
                bytes: bytes.as_ref().clone(),
            })
        } else {
            Ok(())
        };
        let posted = posted.and_then(|()| {
            slot.handle
                .post(&ToWorker::scene(self.scene_id, spec.clone(), &self.tools))
        });
        let posted = posted.and_then(|()| {
            slot.handle.post(&ToWorker::SetEngine {
                scene_id: self.scene_id,
                engine: self.engine,
            })
        });
        match posted {
            Ok(()) => slot.sent_scene_id = Some(self.scene_id),
            Err(error) => {
                slot.state = SlotState::Failed;
                deferred.errors.push(error.clone());
                deferred.gpu_events.push(GpuEvent::Lost { reason: error });
            }
        }
    }

    /// Whether the current scene is ineligible for the GPU, remembered per scene (it
    /// resolves the material, which is too much work for every Worker message).
    pub(super) fn gpu_verdict(&mut self) -> Option<&'static str> {
        let max_storage_binding = self.gpu.as_ref().map_or(0, |g| g.max_storage_binding);
        let key = VerdictKey {
            scene_id: self.scene_id,
            hdr_texels: self.hdr_texels,
            max_storage_binding,
        };
        if let Some((cached, verdict)) = self.verdict_cache
            && cached == key
        {
            return verdict;
        }
        let verdict = self.spec.as_ref().and_then(|spec| {
            gpu_admission::engine_ineligibility_reason(
                self.engine,
                spec,
                self.tools.len(),
                max_storage_binding,
                self.hdr_texels,
                false,
            )
        });
        self.verdict_cache = Some((key, verdict));
        verdict
    }

    /// Hands the GPU Worker the next full pass when it is ready, idle, admitted and the
    /// scene is eligible.
    pub(super) fn schedule_gpu(&mut self, deferred: &mut Deferred) {
        if !(self.gpu_enabled && self.gpu_admitted) || self.spec.is_none() {
            return;
        }
        let scene_id = self.scene_id;
        let free = self.gpu.as_ref().is_some_and(|g| {
            g.state == SlotState::Ready && !g.ledger.is_busy() && g.scene_rejected != Some(scene_id)
        });
        if !free || self.gpu_verdict().is_some() {
            return;
        }
        if self
            .gpu
            .as_ref()
            .is_some_and(|g| g.sent_scene_id != Some(scene_id))
        {
            self.send_scene_to_gpu(deferred, true);
        }
        if self
            .gpu
            .as_ref()
            .is_none_or(|g| g.state != SlotState::Ready)
        {
            return;
        }
        let Some((pass_id, sample_offset, spp)) = self.planner.next_gpu_pass() else {
            return;
        };
        let Some(slot) = self.gpu.as_mut() else {
            self.planner.return_gpu_pass(pass_id);
            return;
        };
        let trace = ToWorker::TraceFullPass {
            scene_id,
            pass_id,
            sample_offset,
            spp,
        };
        match slot.handle.post(&trace) {
            Ok(()) => {
                slot.ledger.begin(LivePass {
                    scene_id,
                    pass_id,
                    sample_offset,
                });
                slot.last_heard_ms = now_ms();
            }
            Err(error) => {
                slot.state = SlotState::Failed;
                self.planner.return_gpu_pass(pass_id);
                deferred
                    .errors
                    .push(format!("could not send full pass to GPU worker: {error}"));
            }
        }
    }
}

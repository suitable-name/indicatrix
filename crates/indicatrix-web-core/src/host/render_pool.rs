//! [`RenderPool`]: the render Workers, the scene they trace, and the page's
//! accumulation buffer.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use glam::Vec3;

use super::{
    cancel_url::CancelUrl,
    handle::WorkerHandle,
    now_ms,
    picture_worker::PictureWorker,
    render_callbacks::{Callbacks, Deferred, finish},
    watchdog::Watchdog,
};
use crate::{
    hdr::{HdrAdmission, admit_hdr},
    protocol::{FromWorker, PictureKind, ToWorker},
    render::{Accumulator, ChunkAssignment, ChunkPlanner, DEFAULT_LIVE_SPP},
    scene::SceneSpec,
};

/// How many times one render Worker is replaced after a crash or a hang.
///
/// After that the pool gives up on it and leaves the restart to the user ("Restart render
/// workers"). The count starts over whenever the Worker delivers a chunk.
pub(super) const MAX_AUTO_RESTARTS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotState {
    /// Waiting for `Loaded`.
    Booting,
    /// `Init` sent, waiting for `Ready`.
    Initializing,
    /// Taking chunks.
    Ready,
    /// Refused `Init`, crashed too often or could not be sent to; takes nothing.
    Failed,
}

/// A chunk handed to a Worker and not yet answered.
pub(super) struct InFlight {
    pub(super) assignment: ChunkAssignment,
    /// Revoked (by dropping it) to tell the Worker to stop: the chunk's scene was replaced
    /// and its result is no longer wanted.
    cancel: Option<CancelUrl>,
}

pub(super) struct Slot {
    pub(super) handle: WorkerHandle,
    /// Distinguishes this Worker's messages from a replaced one's.
    pub(super) spawn_id: u64,
    pub(super) state: SlotState,
    pub(super) busy: bool,
    pub(super) chunk: Option<InFlight>,
    /// When the Worker last spoke, or was last given a chunk.
    pub(super) last_heard_ms: f64,
    /// The scene this Worker was already given a second time after it lost it.
    pub(super) resent_scene: Option<u64>,
    /// Replacements since the Worker last delivered a chunk.
    pub(super) crashes: u32,
}

impl Slot {
    /// A Worker that has just been spawned and is still booting.
    fn new(handle: WorkerHandle, spawn_id: u64) -> Self {
        Self {
            handle,
            spawn_id,
            state: SlotState::Booting,
            busy: false,
            chunk: None,
            last_heard_ms: now_ms(),
            resent_scene: None,
            crashes: 0,
        }
    }
}

pub(super) struct Inner {
    self_weak: Weak<RefCell<Self>>,
    callbacks: Rc<Callbacks>,
    default_workers: u32,
    pub(super) slots: Vec<Slot>,
    /// The Worker that makes pictures, spawned on the first request.
    pub(super) picture_worker: Option<PictureWorker>,
    retired: Vec<WorkerHandle>,
    next_spawn_id: u64,
    pub(super) scene_id: u64,
    pub(super) spec: Option<SceneSpec>,
    pub(super) hdr: Option<(u64, Rc<Vec<u8>>)>,
    pub(super) accumulator: Rc<RefCell<Accumulator>>,
    pub(super) planner: ChunkPlanner,
    target_spp: u32,
    running: bool,
    pub(super) scene_failed: bool,
    /// Finds Workers that went quiet while holding a chunk; held only to keep its timer
    /// running until the pool is dropped.
    _watchdog: Option<Watchdog>,
}

/// The render Workers and the page's full-frame accumulation. A cheap `Rc` handle.
///
/// Partition `i` of the frame is always traced by render Worker `i`, so the partition
/// count is the Worker count; see [`crate::render`] for how passes merge. Pictures (the
/// denoise, an export's PNG) are made by one more Worker of their own, so a tracing
/// Worker never stops tracing for one.
#[derive(Clone)]
pub struct RenderPool {
    inner: Rc<RefCell<Inner>>,
    callbacks: Rc<Callbacks>,
}

impl RenderPool {
    /// Spawns `workers` render Workers (see [`super::WorkerPool::new`]).
    pub(super) fn new(workers: u32) -> Result<Self, String> {
        let callbacks = Rc::new(Callbacks::default());
        let inner = Rc::new_cyclic(|weak| {
            RefCell::new(Inner {
                self_weak: weak.clone(),
                callbacks: Rc::clone(&callbacks),
                default_workers: workers,
                slots: Vec::new(),
                picture_worker: None,
                retired: Vec::new(),
                next_spawn_id: 0,
                scene_id: 0,
                spec: None,
                hdr: None,
                accumulator: Rc::new(RefCell::new(Accumulator::new(0, 0, 0, workers))),
                planner: ChunkPlanner::new(workers, 0, DEFAULT_LIVE_SPP),
                target_spp: DEFAULT_LIVE_SPP,
                running: false,
                scene_failed: false,
                _watchdog: start_watchdog(weak, &callbacks),
            })
        });
        inner.borrow_mut().resize(workers)?;
        Ok(Self { inner, callbacks })
    }

    /// Calls `callback` after every merged pass with the accumulator (read
    /// `sum()` / `sample_count()` and tonemap). Replaces any earlier callback.
    pub fn on_progress(&self, callback: impl FnMut(&Accumulator) + 'static) {
        *self.callbacks.progress.borrow_mut() = Some(Box::new(callback));
    }

    /// Calls `callback` with a readable message for a Worker, scene or HDR failure.
    /// Without one, failures go to the browser console.
    pub fn on_error(&self, callback: impl FnMut(&str) + 'static) {
        *self.callbacks.error.borrow_mut() = Some(Box::new(callback));
    }

    /// Replaces the scene: a new scene id, a fresh accumulator and planner (keeping the
    /// measured Worker speeds), and the spec broadcast to every ready Worker. Chunks still
    /// in flight for the old scene are told to stop (a Worker looks between row groups);
    /// whatever arrives for it is dropped. Keeps running if it was running.
    pub fn set_scene(&self, spec: SceneSpec) {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.retired.clear();
            inner.spec = Some(spec);
            inner.restart_scene()
        };
        self.finish(deferred);
    }

    /// Loads an HDR map on every render Worker under `id` (name it in
    /// `SceneSpec::hdr_id` and call [`Self::set_scene`] afterwards). The page calls it
    /// through [`super::WorkerPool::set_hdr`], which also gives the map to the analysis
    /// Worker.
    ///
    /// The map is never downsampled: when `copies x per-copy memory` would pass the
    /// 1.5 GiB budget, render Workers are stopped until it fits. `analysis_copy` says the
    /// analysis Worker will hold a copy too, which counts as one of the copies (see
    /// [`admit_hdr`]). Returns the admission, whose `workers` is the new render-Worker
    /// count and whose `analysis_copy` says whether the analysis Worker's copy fits. A
    /// scene still naming an older map cannot be built any more; tracing it waits for the
    /// caller's [`Self::set_scene`].
    ///
    /// # Errors
    ///
    /// The refusal text (file too large, not a Radiance file, more texels than the
    /// browser limit, or too large for even one render Worker). The pool is unchanged then.
    pub(super) fn set_hdr(
        &self,
        id: u64,
        bytes: &Rc<Vec<u8>>,
        analysis_copy: bool,
    ) -> Result<HdrAdmission, String> {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.retired.clear();
            let admission = admit_hdr(bytes, inner.default_workers, analysis_copy)
                .map_err(|e| e.to_string())?;
            let bytes = Rc::clone(bytes);
            inner.hdr = Some((id, Rc::clone(&bytes)));
            let resized = inner.slots.len() != admission.workers as usize;
            inner.resize(admission.workers)?;
            let mut deferred = Deferred::default();
            for slot in inner.slots.iter().filter(|s| s.state == SlotState::Ready) {
                let message = ToWorker::HdrMap {
                    id,
                    bytes: bytes.as_ref().clone(),
                };
                if let Err(error) = slot.handle.post(&message) {
                    deferred.errors.push(error);
                }
            }
            if resized && inner.spec.is_some() {
                deferred.absorb(inner.restart_after_hdr_change(Some(id)));
            }
            (admission, deferred)
        };
        let (admission, deferred) = deferred;
        self.finish(deferred);
        Ok(admission)
    }

    /// Drops the HDR map and restores the full render-Worker count. A scene still naming
    /// the map fails on the Workers until [`Self::set_scene`] gives one without it. The
    /// page calls it through [`super::WorkerPool::clear_hdr`].
    ///
    /// # Errors
    ///
    /// When a replacement Worker cannot be started.
    pub(super) fn clear_hdr(&self) -> Result<(), String> {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.retired.clear();
            inner.hdr = None;
            let mut deferred = Deferred::default();
            for slot in inner.slots.iter().filter(|s| s.state == SlotState::Ready) {
                if let Err(error) = slot.handle.post(&ToWorker::ClearHdr) {
                    deferred.errors.push(error);
                }
            }
            let target = inner.default_workers;
            let resized = inner.slots.len() != target as usize;
            inner.resize(target)?;
            if resized && inner.spec.is_some() {
                deferred.absorb(inner.restart_after_hdr_change(None));
            }
            deferred
        };
        self.finish(deferred);
        Ok(())
    }

    /// Starts (or continues) tracing the current scene until `target_spp` samples per
    /// pixel (clamp it with `render::clamp_live_spp` / `clamp_export_spp` first).
    pub fn start(&self, target_spp: u32) {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.target_spp = target_spp;
            inner.planner.set_target_spp(target_spp);
            inner.running = true;
            inner.schedule()
        };
        self.finish(deferred);
    }

    /// Stops handing out chunks. Chunks already in flight still merge; [`Self::start`]
    /// continues where this stopped.
    pub fn cancel(&self) {
        self.inner.borrow_mut().running = false;
    }

    /// Whether chunks are being handed out.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.inner.borrow().running
    }

    /// `true` once the current scene reached its target and every pass is merged.
    #[must_use]
    pub fn is_done(&self) -> bool {
        let inner = self.inner.borrow();
        let passes = inner.accumulator.borrow().completed_passes();
        inner.planner.is_done(passes)
    }

    /// The current scene id.
    #[must_use]
    pub fn scene_id(&self) -> u64 {
        self.inner.borrow().scene_id
    }

    /// The render-Worker count (the partition count of new scenes).
    #[must_use]
    pub fn worker_count(&self) -> u32 {
        self.inner.borrow().slots.len() as u32
    }

    /// The current scene's accumulator (replaced, not reset, by [`Self::set_scene`]).
    #[must_use]
    pub fn accumulator(&self) -> Rc<RefCell<Accumulator>> {
        Rc::clone(&self.inner.borrow().accumulator)
    }

    /// Calls `callback` with every finished [`Self::request_picture`] (replaces any
    /// earlier callback). Results for an older scene still arrive; compare
    /// `scene_id`.
    pub fn on_picture(&self, callback: impl FnMut(super::PictureResult) + 'static) {
        *self.callbacks.picture.borrow_mut() = Some(Box::new(callback));
    }

    /// Sends the current scene's running sum to the picture Worker to make `kind` from
    /// it (the settled live view's denoise, or an export PNG), so neither the page nor a
    /// tracing Worker runs that work. Returns the scene id the result will carry.
    ///
    /// # Errors
    ///
    /// When there is no scene, no sample yet, the picture Worker cannot be started, or a
    /// post fails.
    pub fn request_picture(&self, kind: PictureKind) -> Result<u64, String> {
        let mut inner = self.inner.borrow_mut();
        inner.retired.clear();
        let scene_id = inner.scene_id;
        let Some(spec) = inner.spec.clone() else {
            return Err("there is no scene to make a picture of".to_string());
        };
        let (sample_count, sums) = {
            let accumulator = inner.accumulator.borrow();
            if accumulator.sample_count() == 0 {
                return Err("nothing has been rendered yet".to_string());
            }
            let sums: Vec<[f32; 3]> = accumulator.sum().iter().map(Vec3::to_array).collect();
            (accumulator.sample_count(), sums)
        };
        inner.ensure_picture_worker()?;
        let worker = inner
            .picture_worker
            .as_mut()
            .ok_or("the picture worker is not running")?;
        worker.request(scene_id, &spec, sample_count, sums, kind)?;
        Ok(scene_id)
    }

    /// Render Workers that crashed too often or refused to start. Such a pool cannot
    /// finish a pass until [`Self::restart_workers`].
    #[must_use]
    pub fn failed_worker_count(&self) -> u32 {
        self.inner
            .borrow()
            .slots
            .iter()
            .filter(|s| s.state == SlotState::Failed)
            .count() as u32
    }

    /// Terminates every render Worker and starts the same number afresh (the full
    /// count when none are left), then restarts the current scene on them; the HDR
    /// map, if any, is sent to each new Worker as it becomes ready. The picture Worker
    /// is replaced on its next request.
    ///
    /// # Errors
    ///
    /// When a Worker cannot be started.
    pub fn restart_workers(&self) -> Result<(), String> {
        let deferred = {
            let mut inner = self.inner.borrow_mut();
            inner.retired.clear();
            let count = match inner.slots.len() as u32 {
                0 => inner.default_workers,
                n => n,
            };
            inner.drop_picture_worker();
            inner.resize(0)?;
            inner.resize(count)?;
            if inner.spec.is_some() {
                inner.restart_scene()
            } else {
                Deferred::default()
            }
        };
        self.finish(deferred);
        Ok(())
    }

    /// Runs the deferred callbacks with no borrow of the pool held.
    fn finish(&self, deferred: Deferred) {
        finish(&self.callbacks, deferred);
    }
}

/// The silence watchdog: every few seconds, Workers that went quiet while holding a
/// chunk are replaced.
fn start_watchdog(weak: &Weak<RefCell<Inner>>, callbacks: &Rc<Callbacks>) -> Option<Watchdog> {
    let weak = weak.clone();
    let callbacks = Rc::clone(callbacks);
    Watchdog::start(move || {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let deferred = inner.borrow_mut().check_silence(now_ms());
        finish(&callbacks, deferred);
    })
}

impl Inner {
    /// Grows or shrinks the pool to `count` Workers (stopped ones are parked in
    /// `retired` until the next public call).
    fn resize(&mut self, count: u32) -> Result<(), String> {
        while self.slots.len() > count as usize {
            if let Some(slot) = self.slots.pop() {
                slot.handle.terminate();
                self.retired.push(slot.handle);
            }
        }
        while self.slots.len() < count as usize {
            let index = self.slots.len() as u32;
            let spawn_id = self.next_spawn_id;
            self.next_spawn_id += 1;
            let handle = spawn_render_worker(&self.self_weak, &self.callbacks, index, spawn_id)?;
            self.slots.push(Slot::new(handle, spawn_id));
        }
        Ok(())
    }

    /// Terminates the Worker of slot `index` and starts a fresh one in its place (still
    /// booting). The old handle is parked, never dropped: this may run from its own
    /// callback.
    pub(super) fn replace_worker(&mut self, index: usize) -> Result<(), String> {
        if index >= self.slots.len() {
            return Ok(());
        }
        let spawn_id = self.next_spawn_id;
        self.next_spawn_id += 1;
        let handle = spawn_render_worker(&self.self_weak, &self.callbacks, index as u32, spawn_id)?;
        let slot = &mut self.slots[index];
        let old = std::mem::replace(&mut slot.handle, handle);
        old.terminate();
        slot.spawn_id = spawn_id;
        slot.state = SlotState::Booting;
        slot.busy = false;
        slot.chunk = None;
        slot.last_heard_ms = now_ms();
        self.retired.push(old);
        Ok(())
    }

    /// A new scene id with the current spec: fresh accumulator and planner, spec sent to
    /// every ready Worker, then scheduling. Chunks in flight for the old scene are told
    /// to stop.
    pub(super) fn restart_scene(&mut self) -> Deferred {
        for slot in &mut self.slots {
            if let Some(chunk) = slot.chunk.as_mut() {
                // Dropping the URL revokes it, which is the Worker's signal.
                drop(chunk.cancel.take());
            }
        }
        self.scene_id += 1;
        self.scene_failed = false;
        let partitions = self.slots.len().max(1) as u32;
        let (width, height) = self
            .spec
            .as_ref()
            .map_or((0, 0), |spec| (spec.width, spec.height));
        self.accumulator = Rc::new(RefCell::new(Accumulator::new(
            self.scene_id,
            width,
            height,
            partitions,
        )));
        self.planner = ChunkPlanner::with_rates_from(
            &self.planner,
            partitions,
            width * height,
            self.target_spp,
        );
        let mut deferred = Deferred::default();
        if let Some(spec) = &self.spec {
            let message = ToWorker::SetScene {
                scene_id: self.scene_id,
                spec: spec.clone(),
            };
            for slot in self.slots.iter().filter(|s| s.state == SlotState::Ready) {
                if let Err(error) = slot.handle.post(&message) {
                    deferred.errors.push(error);
                }
            }
        }
        deferred.absorb(self.schedule());
        deferred
    }

    /// After an HDR map was loaded (`held = Some(id)`) or dropped (`None`) on a resized
    /// pool: restarts the current scene, unless it names a map the Workers no longer hold.
    ///
    /// Such a scene cannot be built, and restarting it would only make every Worker
    /// report "HDR map not loaded" just before the caller's `set_scene` replaces it. So
    /// it is left stopped, silently, until that call.
    fn restart_after_hdr_change(&mut self, held: Option<u64>) -> Deferred {
        let names_other_map = self
            .spec
            .as_ref()
            .and_then(|spec| spec.hdr_id)
            .is_some_and(|wanted| Some(wanted) != held);
        if names_other_map {
            self.scene_failed = true;
            Deferred::default()
        } else {
            self.restart_scene()
        }
    }

    /// Hands a chunk to every ready, idle Worker that has one coming. A chunk that
    /// cannot be posted goes back to the planner and its Worker is marked failed.
    pub(super) fn schedule(&mut self) -> Deferred {
        let mut deferred = Deferred::default();
        if !self.running || self.scene_failed || self.spec.is_none() {
            return deferred;
        }
        let completed = self.accumulator.borrow().completed_passes();
        let partitions = self.accumulator.borrow().partitions() as usize;
        for index in 0..self.slots.len().min(partitions) {
            let slot = &self.slots[index];
            if slot.state != SlotState::Ready || slot.busy {
                continue;
            }
            let Some(assignment) = self.planner.next_chunk(index as u32, completed) else {
                continue;
            };
            let cancel = CancelUrl::create();
            let watch = cancel.as_ref().map(|cancel| ToWorker::WatchCancel {
                job_id: self.scene_id,
                url: cancel.url().to_owned(),
            });
            let trace = ToWorker::TraceChunk {
                scene_id: self.scene_id,
                first_pixel: assignment.first_pixel,
                stride: assignment.stride,
                sample_offset: assignment.sample_offset,
                spp: assignment.spp,
            };
            let slot = &mut self.slots[index];
            let posted = match &watch {
                Some(watch) => slot.handle.post(watch),
                None => Ok(()),
            }
            .and_then(|()| slot.handle.post(&trace));
            match posted {
                Ok(()) => {
                    slot.busy = true;
                    slot.last_heard_ms = now_ms();
                    slot.chunk = Some(InFlight { assignment, cancel });
                }
                Err(error) => {
                    slot.state = SlotState::Failed;
                    self.planner.unassign(&assignment);
                    deferred.errors.push(format!(
                        "could not send a chunk to render worker {index}: {error}"
                    ));
                }
            }
        }
        deferred
    }

    /// Starts the picture Worker unless a working one exists.
    fn ensure_picture_worker(&mut self) -> Result<(), String> {
        if self.picture_worker.as_ref().is_some_and(|w| !w.broken) {
            return Ok(());
        }
        self.drop_picture_worker();
        let spawn_id = self.next_spawn_id;
        self.next_spawn_id += 1;
        self.picture_worker = Some(spawn_picture_worker(
            &self.self_weak,
            &self.callbacks,
            spawn_id,
            self.default_workers,
        )?);
        Ok(())
    }

    /// Stops the picture Worker, parking its handle (this may run from its own callback).
    pub(super) fn drop_picture_worker(&mut self) {
        if let Some(worker) = self.picture_worker.take() {
            worker.terminate();
            self.retired.push(worker.into_handle());
        }
    }
}

fn spawn_render_worker(
    weak: &Weak<RefCell<Inner>>,
    callbacks: &Rc<Callbacks>,
    index: u32,
    spawn_id: u64,
) -> Result<WorkerHandle, String> {
    let on_message = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: FromWorker| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner
                .borrow_mut()
                .on_message(index as usize, spawn_id, message);
            finish(&callbacks, deferred);
        }
    };
    let on_error = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: String| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner
                .borrow_mut()
                .on_crash(index as usize, spawn_id, &message);
            finish(&callbacks, deferred);
        }
    };
    WorkerHandle::spawn(on_message, on_error)
}

fn spawn_picture_worker(
    weak: &Weak<RefCell<Inner>>,
    callbacks: &Rc<Callbacks>,
    spawn_id: u64,
    worker_index: u32,
) -> Result<PictureWorker, String> {
    let on_message = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: FromWorker| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner.borrow_mut().on_picture_message(spawn_id, message);
            finish(&callbacks, deferred);
        }
    };
    let on_error = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: String| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner.borrow_mut().on_picture_crash(spawn_id, &message);
            finish(&callbacks, deferred);
        }
    };
    PictureWorker::spawn(spawn_id, worker_index, on_message, on_error)
}

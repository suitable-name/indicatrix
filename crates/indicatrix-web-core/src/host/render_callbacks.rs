//! What the render pool tells the page: the callbacks a caller registers.
//!
//! A message handler fills a [`Deferred`] batch while the pool is borrowed; it is delivered
//! only after the borrow is released, so a callback may call any pool method.

use std::{cell::RefCell, rc::Rc};

use crate::{protocol::PictureKind, render::Accumulator};

pub(super) type ProgressCallback = Box<dyn FnMut(&Accumulator)>;
pub(super) type ErrorCallback = Box<dyn FnMut(&str)>;
pub(super) type PictureCallback = Box<dyn FnMut(PictureResult)>;
pub(super) type GpuEventCallback = Box<dyn FnMut(GpuEvent)>;

/// An event emitted by the GPU render worker.
#[derive(Debug, Clone)]
pub enum GpuEvent {
    Adapter {
        vendor: String,
        architecture: String,
        device: String,
        description: String,
        backend: String,
        max_storage_buffers_per_stage: u32,
        max_storage_binding: u64,
        max_buffer: u64,
    },
    Unavailable {
        reason: String,
    },
    Compiled {
        kernel: String,
        class: u32,
        ms: f64,
    },
    CompileFailed {
        kernel: String,
        class: u32,
        reason: String,
        ms: f64,
    },
    FullPassResult {
        scene_id: u64,
        pass_id: u64,
        sample_offset: u32,
        spp: u32,
        sums_valid: bool,
        elapsed_ms: f64,
    },
    Lost {
        reason: String,
    },
    /// The GPU Worker answered a pass with "no such scene" (the pool already took care of
    /// a pass it wanted; the page uses this to end a self-check that cannot finish).
    PassDropped {
        scene_id: u64,
    },
    /// The GPU Worker could not build a scene.
    SceneRejected {
        scene_id: u64,
        message: String,
    },
}

/// A finished [`super::RenderPool::request_picture`].
#[derive(Debug, Clone)]
pub struct PictureResult {
    /// The scene the sum belonged to (compare with [`super::RenderPool::scene_id`]).
    pub scene_id: u64,
    /// Samples per pixel in the sum it was made from.
    pub sample_count: u32,
    /// What was asked for.
    pub kind: PictureKind,
    /// RGBA8 or PNG bytes (see [`PictureKind`]), or why the Worker could not make it.
    pub bytes: Result<Vec<u8>, String>,
    /// Time it took in the Worker (0 on failure).
    pub elapsed_ms: f64,
}

#[derive(Default)]
pub(super) struct Callbacks {
    pub(super) progress: RefCell<Option<ProgressCallback>>,
    pub(super) error: RefCell<Option<ErrorCallback>>,
    pub(super) picture: RefCell<Option<PictureCallback>>,
    pub(super) gpu_event: RefCell<Option<GpuEventCallback>>,
}

impl Callbacks {
    fn picture(&self, result: PictureResult) {
        let taken = self.picture.borrow_mut().take();
        if let Some(mut callback) = taken {
            callback(result);
            let mut slot = self.picture.borrow_mut();
            if slot.is_none() {
                *slot = Some(callback);
            }
        }
    }

    /// Calls the progress callback with `accumulator` (taken out while it runs, so the
    /// callback may replace itself).
    fn progress(&self, accumulator: &Rc<RefCell<Accumulator>>) {
        let taken = self.progress.borrow_mut().take();
        if let Some(mut callback) = taken {
            callback(&accumulator.borrow());
            let mut slot = self.progress.borrow_mut();
            if slot.is_none() {
                *slot = Some(callback);
            }
        }
    }

    fn error(&self, message: &str) {
        let taken = self.error.borrow_mut().take();
        if let Some(mut callback) = taken {
            callback(message);
            let mut slot = self.error.borrow_mut();
            if slot.is_none() {
                *slot = Some(callback);
            }
        } else {
            web_sys::console::error_1(&format!("render pool: {message}").into());
        }
    }

    fn gpu_event(&self, event: GpuEvent) {
        let taken = self.gpu_event.borrow_mut().take();
        if let Some(mut callback) = taken {
            callback(event);
            let mut slot = self.gpu_event.borrow_mut();
            if slot.is_none() {
                *slot = Some(callback);
            }
        }
    }
}

/// What a message handler wants done once the pool's borrow is released.
#[derive(Default)]
pub(super) struct Deferred {
    pub(super) progress: Option<Rc<RefCell<Accumulator>>>,
    pub(super) errors: Vec<String>,
    pub(super) pictures: Vec<PictureResult>,
    pub(super) gpu_events: Vec<GpuEvent>,
}

impl Deferred {
    /// Adds `other`'s errors and pictures after this batch's own (its progress, if this
    /// batch has none, is kept too).
    pub(super) fn absorb(&mut self, other: Self) {
        self.errors.extend(other.errors);
        self.pictures.extend(other.pictures);
        self.gpu_events.extend(other.gpu_events);
        if self.progress.is_none() {
            self.progress = other.progress;
        }
    }
}

/// Runs the deferred callbacks with no borrow of the pool held.
pub(super) fn finish(callbacks: &Callbacks, deferred: Deferred) {
    for error in &deferred.errors {
        callbacks.error(error);
    }
    if let Some(accumulator) = deferred.progress {
        callbacks.progress(&accumulator);
    }
    for picture in deferred.pictures {
        callbacks.picture(picture);
    }
    for event in deferred.gpu_events {
        callbacks.gpu_event(event);
    }
}

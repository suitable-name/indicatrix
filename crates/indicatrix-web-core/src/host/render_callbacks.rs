//! What the render pool tells the page: the callbacks a caller registers.
//!
//! A message handler fills a [`Deferred`] batch while the pool is borrowed; it is delivered
//! only after the borrow is released, so a callback may call any pool method.

use std::{cell::RefCell, rc::Rc};

use crate::{protocol::PictureKind, render::Accumulator};

pub(super) type ProgressCallback = Box<dyn FnMut(&Accumulator)>;
pub(super) type ErrorCallback = Box<dyn FnMut(&str)>;
pub(super) type PictureCallback = Box<dyn FnMut(PictureResult)>;

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
}

/// What a message handler wants done once the pool's borrow is released.
#[derive(Default)]
pub(super) struct Deferred {
    pub(super) progress: Option<Rc<RefCell<Accumulator>>>,
    pub(super) errors: Vec<String>,
    pub(super) pictures: Vec<PictureResult>,
}

impl Deferred {
    /// Adds `other`'s errors and pictures after this batch's own (its progress, if this
    /// batch has none, is kept too).
    pub(super) fn absorb(&mut self, other: Self) {
        self.errors.extend(other.errors);
        self.pictures.extend(other.pictures);
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
}

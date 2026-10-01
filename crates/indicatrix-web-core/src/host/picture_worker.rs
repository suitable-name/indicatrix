//! [`PictureWorker`]: a render-role Worker that only makes pictures, so none of the
//! tracing Workers has to stop tracing for one.
//!
//! Pictures are the settled live view's denoise and an export's tone map and PNG encode.
//!
//! Partition `i` of a frame is always traced by render Worker `i`, and a pass needs every
//! partition, so a tracing Worker busy with a denoise would hold up every following pass.
//! The picture Worker is spawned on first use, is sent the current scene (without its HDR
//! map, which a picture never reads) whenever the scene it last held is not the one a
//! sum belongs to, and answers pictures in the order they were asked for.

use std::collections::VecDeque;

use super::{
    handle::WorkerHandle,
    render_callbacks::{Deferred, PictureResult},
};
use crate::{
    protocol::{FromWorker, PROTOCOL_VERSION, PictureKind, ToWorker, WorkerRole},
    scene::SceneSpec,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waiting for `Loaded`.
    Booting,
    /// `Init` sent, waiting for `Ready`.
    Initializing,
    /// Taking pictures.
    Ready,
}

/// The picture Worker and the pictures it owes.
pub(super) struct PictureWorker {
    handle: WorkerHandle,
    /// Distinguishes this Worker's messages from a replaced one's.
    pub(super) spawn_id: u64,
    /// The index sent in `Init` (no tracing partition has it).
    worker_index: u32,
    stage: Stage,
    /// Messages asked for before `Ready`, posted in order once it arrives (a Worker
    /// loses messages posted before its handler exists).
    outbox: Vec<ToWorker>,
    /// The scene id the Worker was last sent.
    scene_sent: Option<u64>,
    /// Pictures asked for and not yet answered, oldest first.
    in_flight: VecDeque<(u64, PictureKind)>,
    /// The Worker refused `Init` or failed: it makes nothing more and is replaced on the
    /// next request.
    pub(super) broken: bool,
}

impl PictureWorker {
    /// Starts the Worker; see [`WorkerHandle::spawn`] for the two callbacks.
    pub(super) fn spawn(
        spawn_id: u64,
        worker_index: u32,
        on_message: impl FnMut(FromWorker) + 'static,
        on_error: impl FnMut(String) + 'static,
    ) -> Result<Self, String> {
        Ok(Self {
            handle: WorkerHandle::spawn(on_message, on_error)?,
            spawn_id,
            worker_index,
            stage: Stage::Booting,
            outbox: Vec::new(),
            scene_sent: None,
            in_flight: VecDeque::new(),
            broken: false,
        })
    }

    /// Stops the Worker at once.
    pub(super) fn terminate(&self) {
        self.handle.terminate();
    }

    /// The Worker's handle, for the pool to park once the Worker is terminated (a handle
    /// must not be dropped from inside its own callbacks).
    pub(super) fn into_handle(self) -> WorkerHandle {
        self.handle
    }

    /// Asks for `kind` made from `sums` (`sample_count` samples per pixel) of scene
    /// `scene_id`, described by `spec`.
    ///
    /// # Errors
    ///
    /// When a message cannot be posted.
    pub(super) fn request(
        &mut self,
        scene_id: u64,
        spec: &SceneSpec,
        sample_count: u32,
        sums: Vec<[f32; 3]>,
        kind: PictureKind,
    ) -> Result<(), String> {
        let mut messages = Vec::with_capacity(2);
        if self.scene_sent != Some(scene_id) {
            messages.push(ToWorker::SetScene {
                scene_id,
                spec: SceneSpec {
                    hdr_id: None,
                    ..spec.clone()
                },
            });
        }
        messages.push(ToWorker::Picture {
            scene_id,
            sample_count,
            sums,
            kind,
        });
        if self.stage == Stage::Ready {
            for message in &messages {
                self.handle.post(message)?;
            }
        } else {
            self.outbox.extend(messages);
        }
        self.scene_sent = Some(scene_id);
        self.in_flight.push_back((scene_id, kind));
        Ok(())
    }

    /// Handles one reply: the boot handshake, a finished or failed picture, or a scene
    /// the Worker could not build (the picture asked for with it then fails too).
    pub(super) fn on_message(&mut self, message: FromWorker) -> Deferred {
        let mut deferred = Deferred::default();
        match message {
            FromWorker::Loaded { .. } => {
                self.stage = Stage::Initializing;
                let init = ToWorker::Init {
                    protocol_version: PROTOCOL_VERSION,
                    role: WorkerRole::Render,
                    worker_index: self.worker_index,
                };
                if let Err(error) = self.handle.post(&init) {
                    deferred.absorb(self.fail(&error));
                }
            }
            FromWorker::Ready { .. } => {
                self.stage = Stage::Ready;
                for message in std::mem::take(&mut self.outbox) {
                    if let Err(error) = self.handle.post(&message) {
                        deferred.absorb(self.fail(&error));
                        break;
                    }
                }
            }
            FromWorker::Picture {
                scene_id,
                sample_count,
                kind,
                bytes,
                elapsed_ms,
            } => {
                self.in_flight.pop_front();
                deferred.pictures.push(PictureResult {
                    scene_id,
                    sample_count,
                    kind,
                    bytes: Ok(bytes),
                    elapsed_ms,
                });
            }
            FromWorker::PictureFailed {
                scene_id,
                kind,
                message,
            } => {
                self.in_flight.pop_front();
                deferred.pictures.push(PictureResult {
                    scene_id,
                    sample_count: 0,
                    kind,
                    bytes: Err(message),
                    elapsed_ms: 0.0,
                });
            }
            // The picture sent after the scene fails by itself ("no longer holds scene").
            FromWorker::SceneError { message, .. } => deferred.errors.push(message),
            FromWorker::Error { message } => deferred.absorb(self.fail(&message)),
            FromWorker::ChunkResult { .. }
            | FromWorker::ChunkDropped { .. }
            | FromWorker::ChunkAborted { .. }
            | FromWorker::HdrLoaded { .. }
            | FromWorker::HdrError { .. }
            | FromWorker::Progress { .. }
            | FromWorker::SolveResult { .. } => {}
        }
        deferred
    }

    /// The Worker crashed or went silent: every picture it owed fails.
    pub(super) fn on_crash(&mut self, message: &str) -> Deferred {
        self.fail(&format!("the picture worker crashed: {message}"))
    }

    /// Marks the Worker broken and fails every picture it owed with `why`.
    fn fail(&mut self, why: &str) -> Deferred {
        self.broken = true;
        self.outbox.clear();
        let mut deferred = Deferred::default();
        for (scene_id, kind) in self.in_flight.drain(..) {
            deferred.pictures.push(PictureResult {
                scene_id,
                sample_count: 0,
                kind,
                bytes: Err(why.to_string()),
                elapsed_ms: 0.0,
            });
        }
        deferred.errors.push(why.to_string());
        deferred
    }
}

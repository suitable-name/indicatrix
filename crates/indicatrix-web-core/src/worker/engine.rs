//! Per-scene renderer selection, separate from stable scene payloads.

use indicatrix::renderer::engine::EngineSpec;

use super::{FromWorker, WorkerHandler, WorkerRole};

impl WorkerHandler {
    pub(super) fn set_engine(&mut self, scene_id: u64, engine: EngineSpec) -> Option<FromWorker> {
        if let Err(reply) = self.require(WorkerRole::Render, "SetEngine") {
            return Some(*reply);
        }
        let Some(cached) = self
            .scene
            .as_mut()
            .filter(|cached| cached.scene_id == scene_id)
        else {
            return Some(FromWorker::SceneError {
                scene_id,
                message: "Renderer selection belongs to an unavailable scene.".to_owned(),
            });
        };
        if let Err(error) = cached.scene.set_engine(engine) {
            self.scene = None;
            return Some(FromWorker::SceneError {
                scene_id,
                message: error.to_string(),
            });
        }
        self.denoiser.clear();
        None
    }
}

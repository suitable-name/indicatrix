//! Renderer metadata follows scene messages, before any trace assignment.

use super::{
    handle::WorkerHandle,
    render_pool::{Inner, RenderPool},
};
use crate::protocol::ToWorker;

impl RenderPool {
    /// The renderer of the current accumulation, including its tap options.
    #[must_use]
    pub fn engine(&self) -> indicatrix::renderer::engine::EngineSpec {
        self.inner.borrow().engine
    }
}

impl Inner {
    pub(super) fn post_engine_to(&self, handle: &WorkerHandle) -> Result<(), String> {
        if self.engine.is_default() {
            return Ok(());
        }
        handle.post(&ToWorker::SetEngine {
            scene_id: self.scene_id,
            engine: self.engine,
        })
    }
}

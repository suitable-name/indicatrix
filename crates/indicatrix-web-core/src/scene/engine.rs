//! Engine state deliberately rides beside the unchanged wire `SceneSpec`.

use indicatrix::renderer::{
    engine::EngineSpec,
    whitted::{WhittedDecline, WhittedPrep},
};

use super::OwnedScene;

impl OwnedScene {
    /// Selected renderer and deterministic tap options.
    #[must_use]
    pub const fn engine(&self) -> EngineSpec {
        self.engine
    }

    /// Selects an engine and prepares expensive Ray-Traced inputs once.
    ///
    /// # Errors
    /// UV lamps and fluorescence require the spectral renderer.
    pub fn set_engine(&mut self, mut engine: EngineSpec) -> Result<(), WhittedDecline> {
        engine.ray = engine.ray.sanitized();
        if self.engine != engine {
            self.ray_prep.take();
            self.engine = engine;
        }
        if engine.is_ray() {
            self.whitted_prep()?;
        }
        Ok(())
    }

    /// Prepared tint, lighting and frost; reused across every tap and chunk.
    ///
    /// # Errors
    /// UV lamps and fluorescence require the spectral renderer.
    pub fn whitted_prep(&self) -> Result<&WhittedPrep<'static>, WhittedDecline> {
        self.ray_prep
            .get_or_init(|| {
                WhittedPrep::new(&self.frame_scene(), self.fluorescence(), self.engine.ray).map(
                    |prep| {
                        prep.into_owned_with_hdr(match &self.environment {
                            super::OwnedEnvironment::Hdr(map) => Some(std::sync::Arc::clone(map)),
                            super::OwnedEnvironment::Studio(_) => None,
                        })
                    },
                )
            })
            .as_ref()
            .map_err(|error| *error)
    }
}

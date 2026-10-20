//! Numeric persisted engine choices remain tolerant of settings downgrades.

use indicatrix::renderer::engine::{EngineSpec, RayOptions, RenderEngine};

use super::RenderSettings;

impl RenderSettings {
    /// Typed renderer options without extending the stable scene wire payload.
    #[must_use]
    pub fn engine_spec(&self) -> EngineSpec {
        EngineSpec {
            engine: RenderEngine::from_index(u8::try_from(self.engine).unwrap_or(0)),
            ray: RayOptions {
                aa_taps: u8::try_from(self.ray_taps.clamp(1, 16)).unwrap_or(8),
                fire: self.ray_fire,
            }
            .sanitized(),
        }
    }

    /// Live target; Ray-Traced taps bypass the path tracer's minimum spp.
    #[must_use]
    pub fn live_target(&self) -> u32 {
        self.engine_spec()
            .target_samples(crate::render::clamp_live_spp(self.target_spp))
    }

    /// Export AA is stored separately so changing it never restarts the live view.
    #[must_use]
    pub fn export_engine_spec(&self) -> EngineSpec {
        let mut engine = self.engine_spec();
        engine.ray.aa_taps = u8::try_from(self.export.ray_taps.clamp(1, 16)).unwrap_or(16);
        engine.ray = engine.ray.sanitized();
        engine
    }

    /// Export target; Ray-Traced taps bypass the path tracer's minimum spp.
    #[must_use]
    pub fn export_target(&self) -> u32 {
        self.export_engine_spec()
            .target_samples(crate::render::clamp_export_spp(self.export.spp))
    }
}

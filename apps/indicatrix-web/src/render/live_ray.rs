//! Ray-Traced presentation remains separate from unchanged spectral scene payloads.

use indicatrix::renderer::{engine::EngineSpec, whitted::WhittedDecline};
use indicatrix_web_core::scene::SceneSpec;

use crate::app::Ctx;

pub(super) fn engine(ctx: &Ctx) -> EngineSpec {
    ctx.state.borrow().settings.engine_spec()
}

pub(super) fn validate(ctx: &Ctx, spec: &SceneSpec) -> Result<(), String> {
    if engine(ctx).is_ray() && spec.hdr_id.is_none() && spec.lighting.preset().is_uv_lamp() {
        return Err(WhittedDecline::UvLamp.to_string());
    }
    Ok(())
}

pub(super) fn progress(samples: u32, target: u32) -> String {
    if samples >= target {
        "Ray-Traced · done".to_owned()
    } else {
        format!("Ray-Traced · tap {samples} of {target}")
    }
}

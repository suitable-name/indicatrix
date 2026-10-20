//! Independent eligibility and storage identity for the small deterministic kernel.

use indicatrix::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;

use super::{
    AdapterFacts, REASON_FLUORESCENT, REASON_HDR_TOO_BIG, REASON_TOO_MANY_TOOLS, REASON_UNRESOLVED,
    REASON_UV_LAMP, adapter_key,
};
use crate::scene::{SceneSpec, resolve_scene_material};

/// Ray-Traced compiles one material-independent kernel.
pub const WHITTED_CLASS: u32 = 4;
pub const WHITTED_KERNEL: &str = "whitted";

/// Eligibility for the selected renderer, shared by the page gate and pass scheduler.
#[must_use]
pub fn engine_ineligibility_reason(
    engine: indicatrix::renderer::engine::EngineSpec,
    spec: &SceneSpec,
    tool_count: usize,
    max_storage_binding: u64,
    hdr_texels: Option<u64>,
    fluorescent: bool,
) -> Option<&'static str> {
    let check = if engine.is_ray() {
        whitted_ineligibility_reason
    } else {
        super::gpu_ineligibility_reason_with_tools
    };
    check(
        spec,
        tool_count,
        max_storage_binding,
        hdr_texels,
        fluorescent,
    )
}

/// Every optical material class is admitted; the reduced model uses scalar indices.
#[must_use]
pub fn whitted_ineligibility_reason(
    spec: &SceneSpec,
    tool_count: usize,
    max_storage_binding: u64,
    hdr_texels: Option<u64>,
    fluorescent: bool,
) -> Option<&'static str> {
    let planes: Vec<_> = spec.planes.iter().copied().map(Into::into).collect();
    if resolve_scene_material(&spec.material, &planes).is_err() {
        return Some(REASON_UNRESOLVED);
    }
    if fluorescent {
        return Some(REASON_FLUORESCENT);
    }
    if spec.hdr_id.is_none() && spec.lighting.preset().is_uv_lamp() {
        return Some(REASON_UV_LAMP);
    }
    if spec.hdr_id.is_some()
        && hdr_texels.is_some_and(|texels| {
            max_storage_binding > 0 && texels.saturating_mul(16) > max_storage_binding
        })
    {
        return Some(REASON_HDR_TOO_BIG);
    }
    (tool_count > GPU_MAX_TOOL_PRIMITIVES).then_some(REASON_TOO_MANY_TOOLS)
}

/// Does not alter any existing event-wavefront adapter key or admission record.
#[must_use]
pub fn whitted_adapter_key(facts: &AdapterFacts<'_>, browser_major: &str, hash: &str) -> String {
    adapter_key(facts, browser_major, format!("whitted:{hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_material_is_eligible_including_biaxial_and_zoned() {
        let mut spec = super::super::self_check_scene();
        for material in indicatrix::optics::materials::GemMaterial::built_in() {
            spec.material.name.clone_from(&material.name);
            assert_eq!(
                whitted_ineligibility_reason(&spec, 0, 0, None, false),
                None,
                "{}",
                material.name
            );
        }
        assert_eq!(
            whitted_ineligibility_reason(&spec, GPU_MAX_TOOL_PRIMITIVES + 1, 0, None, false),
            Some(REASON_TOO_MANY_TOOLS)
        );
        assert_eq!(
            whitted_ineligibility_reason(&spec, 0, 0, None, true),
            Some(REASON_FLUORESCENT)
        );
        spec.hdr_id = Some(1);
        assert_eq!(
            whitted_ineligibility_reason(&spec, 0, 16, Some(2), false),
            Some(REASON_HDR_TOO_BIG)
        );
    }

    #[test]
    fn page_gate_and_scheduler_use_the_selected_engines_material_rules() {
        use indicatrix::renderer::engine::{EngineSpec, RenderEngine};
        let mut spec = super::super::self_check_scene();
        let path = EngineSpec::default();
        let ray = EngineSpec {
            engine: RenderEngine::RayTraced,
            ..path
        };
        for name in ["Sapphire", "Peridot", "Ametrine"] {
            spec.material = crate::scene::MaterialSpec::catalogue(name);
            assert!(engine_ineligibility_reason(path, &spec, 0, 0, None, false).is_some());
            assert_eq!(
                engine_ineligibility_reason(ray, &spec, 0, 0, None, false),
                None
            );
        }
        spec.hdr_id = Some(1);
        assert_eq!(
            engine_ineligibility_reason(ray, &spec, 0, 16, Some(2), false),
            Some(REASON_HDR_TOO_BIG)
        );
    }

    #[test]
    fn ray_records_have_separate_identity_without_changing_the_old_key() {
        let facts = AdapterFacts {
            vendor: "a",
            architecture: "b",
            device: "c",
            description: "d",
            backend: "WebGpu",
            max_storage_buffers_per_stage: 8,
            max_storage_binding: 1024,
            max_buffer: 2048,
        };
        let old = adapter_key(&facts, "130", "samehash");
        assert_ne!(whitted_adapter_key(&facts, "130", "samehash"), old);
        assert_eq!(adapter_key(&facts, "130", "samehash"), old);
    }
}

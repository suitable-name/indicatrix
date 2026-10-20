//! Routing for concave stones (a stone with tools): which scenes the GPU traces with its
//! tool-aware kernels and why it declines the rest.
//!
//! Split from the plane-only [`super::scene_routes_to_gpu`] on purpose. That function (and
//! every [`super::GpuSceneRef`], which carries planes only) belongs to callers that cannot
//! hand the GPU a tool list, the mobile backend among them; for them a stone with tools
//! must keep declining, or the GPU would render it without its concave facets. A caller
//! that passes tools through [`super::GpuBackend::try_accumulate_geom`] (or the batch's
//! `GpuBatchItem::tools`) asks [`scene_routes_to_gpu_with_tools`] instead.

use std::fmt;

use super::{Fluorescence, GemMaterial, LightingPreset, StoneGeometry};
use crate::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;

/// Why a scene is not offered to the GPU at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuRouteDecline {
    /// The material has a feature only the CPU supports (`GemMaterial::gpu_supported`).
    Material,
    /// The stone has more concave tools than the GPU kernels trace
    /// ([`GPU_MAX_TOOL_PRIMITIVES`]); the CPU tracer renders it.
    TooManyTools {
        /// Tool count of the stone.
        tools: usize,
    },
    /// The material fluoresces; fluorescence is single-wavelength CPU transport.
    Fluorescence,
    /// A UV lamp lights the scene; those spectra exist only on the CPU.
    UvLamp,
}

impl fmt::Display for GpuRouteDecline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Material => f.write_str("The material has features only the CPU tracer supports"),
            Self::TooManyTools { tools } => write!(
                f,
                "The stone has {tools} concave tools; the GPU traces at most \
                 {GPU_MAX_TOOL_PRIMITIVES}"
            ),
            Self::Fluorescence => f.write_str("Fluorescence is traced on the CPU only"),
            Self::UvLamp => f.write_str("UV lamp lighting is traced on the CPU only"),
        }
    }
}

/// The reason the GPU declines this scene, `None` when it is offered to the GPU.
///
/// The GPU traces concave stones with up to [`GPU_MAX_TOOL_PRIMITIVES`] tools, so unlike
/// [`super::scene_routes_to_gpu`] a stone with tools is routed to it.
///
/// A pure function of the scene so it is unit-tested without an adapter.
#[must_use]
pub fn scene_decline_with_tools(
    material: &GemMaterial,
    geom: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> Option<GpuRouteDecline> {
    if !material.gpu_supported() {
        Some(GpuRouteDecline::Material)
    } else if geom.tools.len() > GPU_MAX_TOOL_PRIMITIVES {
        Some(GpuRouteDecline::TooManyTools {
            tools: geom.tools.len(),
        })
    } else if !fluorescence.is_empty() {
        Some(GpuRouteDecline::Fluorescence)
    } else if lighting.is_uv_lamp() {
        Some(GpuRouteDecline::UvLamp)
    } else {
        None
    }
}

/// Whether a scene may be dispatched to the GPU when the caller passes the stone's tools.
///
/// See the module docs. Same rules as [`super::scene_routes_to_gpu`] except that concave
/// stones with at most [`GPU_MAX_TOOL_PRIMITIVES`] tools are accepted.
#[must_use]
pub fn scene_routes_to_gpu_with_tools(
    material: &GemMaterial,
    geom: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> bool {
    scene_decline_with_tools(material, geom, fluorescence, lighting).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{cuts::StandardGemCuts, tool::ToolPrimitive};
    use crate::optics::{
        absorption::AbsorptionBand,
        fluorescence::{EmissionBand, FluorescentEmitter},
    };
    use glam::Vec3;

    fn tools(n: usize) -> Vec<ToolPrimitive> {
        (0..n)
            .map(|i| {
                let x = 0.01 * i as f32;
                ToolPrimitive::ball(Vec3::new(x, 0.3, 0.0), 0.05)
            })
            .collect()
    }

    fn decline(
        planes: &[crate::geometry::GpuFacetPlane],
        tools: &[ToolPrimitive],
    ) -> Option<GpuRouteDecline> {
        scene_decline_with_tools(
            &GemMaterial::diamond(),
            StoneGeometry { planes, tools },
            Fluorescence::none(),
            LightingPreset::Daylight,
        )
    }

    /// Replaces the old "a stone with tools never routes to the GPU" pin: tools within the
    /// limit route to the GPU, one more than the limit declines with its own reason.
    #[test]
    fn concave_stones_route_to_the_gpu_up_to_the_tool_limit() {
        let planes = StandardGemCuts::standard_round_brilliant();
        assert!(
            GemMaterial::diamond().gpu_supported(),
            "test premise: a supported material"
        );
        assert_eq!(decline(&planes, &[]), None);
        assert_eq!(decline(&planes, &tools(1)), None);
        assert_eq!(decline(&planes, &tools(GPU_MAX_TOOL_PRIMITIVES)), None);
        let over = tools(GPU_MAX_TOOL_PRIMITIVES + 1);
        assert_eq!(
            decline(&planes, &over),
            Some(GpuRouteDecline::TooManyTools {
                tools: GPU_MAX_TOOL_PRIMITIVES + 1
            })
        );
        assert!(scene_routes_to_gpu_with_tools(
            &GemMaterial::diamond(),
            StoneGeometry {
                planes: &planes,
                tools: &tools(3)
            },
            Fluorescence::none(),
            LightingPreset::Daylight
        ));
        assert!(!scene_routes_to_gpu_with_tools(
            &GemMaterial::diamond(),
            StoneGeometry {
                planes: &planes,
                tools: &over
            },
            Fluorescence::none(),
            LightingPreset::Daylight
        ));
    }

    #[test]
    fn the_decline_reason_names_the_limit() {
        let text = GpuRouteDecline::TooManyTools { tools: 65 }.to_string();
        assert!(text.contains("65"), "{text}");
        assert!(
            text.contains(&GPU_MAX_TOOL_PRIMITIVES.to_string()),
            "{text}"
        );
    }

    /// The CPU's own bound is above the GPU's, so the GPU decline is reachable by a stone
    /// the CPU tracer accepts, and it always has a CPU fallback. Both are constants, so this
    /// is a compile-time check.
    const _: () = assert!(GPU_MAX_TOOL_PRIMITIVES <= crate::geometry::tool::MAX_TOOL_PRIMITIVES);

    /// Everything that kept a plane-only stone on the CPU still does with tools present.
    #[test]
    fn fluorescence_and_uv_lamps_still_decline_a_concave_stone() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let t = tools(2);
        let stone = StoneGeometry {
            planes: &planes,
            tools: &t,
        };
        let material = GemMaterial::diamond();
        let emitter = FluorescentEmitter {
            excitation: vec![AbsorptionBand::new(410.0, 20.0, 1.0)],
            emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
            quantum_yield: 0.9,
        };
        let fluorescent = Fluorescence::new(vec![emitter]);
        assert_eq!(
            scene_decline_with_tools(&material, stone, &fluorescent, LightingPreset::Daylight),
            Some(GpuRouteDecline::Fluorescence)
        );
        for preset in LightingPreset::ALL {
            assert_eq!(
                scene_routes_to_gpu_with_tools(&material, stone, Fluorescence::none(), preset),
                !preset.is_uv_lamp(),
                "{preset:?}"
            );
        }
    }

    /// For a plane-only stone the two routing functions agree on every preset; the legacy
    /// one still declines any tools (callers that cannot pass them).
    #[test]
    fn the_legacy_routing_agrees_without_tools_and_still_declines_tools() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        for preset in LightingPreset::ALL {
            assert_eq!(
                super::super::scene_routes_to_gpu(
                    &material,
                    StoneGeometry::planes_only(&planes),
                    Fluorescence::none(),
                    preset
                ),
                scene_routes_to_gpu_with_tools(
                    &material,
                    StoneGeometry::planes_only(&planes),
                    Fluorescence::none(),
                    preset
                ),
                "{preset:?}"
            );
        }
        let t = tools(1);
        assert!(!super::super::scene_routes_to_gpu(
            &material,
            StoneGeometry {
                planes: &planes,
                tools: &t
            },
            Fluorescence::none(),
            LightingPreset::Daylight
        ));
    }
}

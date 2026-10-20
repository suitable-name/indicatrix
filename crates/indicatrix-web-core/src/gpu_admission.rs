//! The pure half of the GPU admission gate (plan section 6.5).
//!
//! It decides which scenes the GPU may trace and holds the stored per-adapter record and the
//! adapter key. No DOM, no Workers, so all of it is tested natively; the page's
//! `render::gpu_gate` and the pool in [`crate::host`] call it.
//!
//! - [`gpu_ineligibility_reason`]: why a scene cannot go to the GPU in v1 (isotropic, not
//!   zoned, not fluorescent, no UV lamp, the HDR map fits the adapter);
//!   [`gpu_ineligibility_reason_with_tools`] adds the concave-tool limit.
//! - [`AdmissionRecord`] / [`AdmissionState`]: what is stored per adapter, and
//!   [`find_probing_key`], the crash-marker scan.
//! - [`adapter_key`]: the FNV-1a hash that names the record.
//! - [`self_check_scene`] and the compile deadlines the gate uses.

use std::{collections::BTreeMap, fmt::Display};
mod probe;
pub use probe::ProbeRecord;
mod whitted;
pub use whitted::{
    WHITTED_CLASS, WHITTED_KERNEL, engine_ineligibility_reason, whitted_adapter_key,
    whitted_ineligibility_reason,
};

use indicatrix::{
    geometry::{cuts::StandardGemCuts, tool::GPU_MAX_TOOL_PRIMITIVES},
    optics::{materials::CrystalSystem, raytracer::LightingPreset},
    render_setup::Backdrop,
};
use serde::{Deserialize, Serialize};

use crate::{
    scene::{
        CameraSpec, FinishSpec, LightingSpec, MaterialSpec, SceneSpec, planes_to_data,
        resolve_scene_material,
    },
    storage_keys,
};

/// The full v1 isotropic kernel set in smallest-first order (section 6.5).
pub const ISOTROPIC_KERNELS_ORDER: &[&str] = &[
    "Tail",
    "ArmTir",
    "SplitResolve",
    "Finalize",
    "Generate",
    "Surface",
    "Extend",
    "ArmGeneric",
    "Scatter",
    "NeeHg",
    "ArmFrosted",
];

/// Dedicated scene id of the 16x16 self-check render.
pub const SELF_CHECK_SCENE_ID: u64 = 0xffff_ffff_ffff_ffff;
/// Samples per pixel of the self-check pass.
pub const SELF_CHECK_SPP: u32 = 4;
/// The material class the v1 kernels are compiled for (1 = isotropic).
pub const ISOTROPIC_CLASS: u32 = 1;
/// The worker's soft compile limit: a kernel slower than this fails the test.
pub const COMPILE_SOFT_MS: u32 = 5_000;
/// The worker's hard compile limit per request.
pub const COMPILE_HARD_MS: u32 = 20_000;
/// How long the page waits for the next answer of the worker while a test or an admission
/// compile runs (reset on every answer). Past it the page gives the worker up.
pub const STAGE_DEADLINE_MS: u32 = COMPILE_HARD_MS + 10_000;

/// Reason text: the material did not resolve.
pub const REASON_UNRESOLVED: &str = "material unresolved";
/// Reason text: the stone is birefringent.
pub const REASON_ANISOTROPIC: &str = "anisotropic stones not yet supported";
/// Reason text: the stone has color zones.
pub const REASON_ZONED: &str = "color-zoned stones not supported on GPU";
/// Reason text: the material fluoresces.
pub const REASON_FLUORESCENT: &str = "fluorescent stones not supported on GPU";
/// Reason text: the lighting is a UV lamp.
pub const REASON_UV_LAMP: &str = "UV lamp lighting not supported on GPU";
/// Reason text: the HDR map does not fit the adapter's storage binding.
pub const REASON_HDR_TOO_BIG: &str = "HDR map exceeds GPU memory limits";
/// Reason text: the stone has more concave tools than the GPU kernels trace
/// (`indicatrix::geometry::tool::GPU_MAX_TOOL_PRIMITIVES`, 128; a test keeps the number in the
/// text equal to the constant).
pub const REASON_TOO_MANY_TOOLS: &str = "more than 128 concave tools (the GPU traces at most 128)";

/// Why `spec` is not eligible for WebGPU event-wavefront rendering in v1 (`None` = eligible).
///
/// `fluorescent` is whether the scene's material carries fluorescence. A [`SceneSpec`]
/// carries no physics recipe and neither web worker builds fluorescence today, so callers
/// pass `false`; the rule is here so that a spec that learns to carry one cannot reach the
/// GPU by accident.
#[must_use]
pub fn gpu_ineligibility_reason(
    spec: &SceneSpec,
    max_storage_binding: u64,
    hdr_texels: Option<u64>,
    fluorescent: bool,
) -> Option<&'static str> {
    let planes: Vec<indicatrix::geometry::GpuFacetPlane> =
        spec.planes.iter().copied().map(Into::into).collect();
    let Ok(material) = resolve_scene_material(&spec.material, &planes) else {
        return Some(REASON_UNRESOLVED);
    };
    // Mirrors `renderer::gpu::frame::specialisation::classify_material` == ISOTROPIC.
    let is_isotropic = material.biaxial_delta_beta_alpha.is_none()
        && (material.crystal_system == CrystalSystem::Cubic
            || material.birefringence_delta.abs() <= 1e-4);
    if !is_isotropic {
        return Some(REASON_ANISOTROPIC);
    }
    if material.zoning.is_some() {
        return Some(REASON_ZONED);
    }
    if fluorescent {
        return Some(REASON_FLUORESCENT);
    }
    if spec.lighting.preset().is_uv_lamp() {
        return Some(REASON_UV_LAMP);
    }
    if spec.hdr_id.is_some()
        && let Some(texels) = hdr_texels
        && max_storage_binding > 0
        && texels.saturating_mul(16) > max_storage_binding
    {
        return Some(REASON_HDR_TOO_BIG);
    }
    None
}

/// [`gpu_ineligibility_reason`] for a stone with `tool_count` concave tools.
///
/// The tools come from a [`GeometrySceneSpec`](crate::scene::GeometrySceneSpec) or are sent
/// with [`ToWorker::SetGeometryScene`](crate::protocol::ToWorker::SetGeometryScene).
///
/// The event-wavefront kernels trace concave stones (the tools ride in the `planes` storage
/// binding, `renderer::gpu::event_wavefront::EventWavefrontRenderer::accumulate_with_tools`)
/// up to `GPU_MAX_TOOL_PRIMITIVES`; beyond that the stone is the CPU workers' alone. Every
/// other reason keeps its precedence, so a stone that is both anisotropic and tool-heavy
/// still names the material. With `tool_count == 0` this is exactly
/// [`gpu_ineligibility_reason`].
#[must_use]
pub fn gpu_ineligibility_reason_with_tools(
    spec: &SceneSpec,
    tool_count: usize,
    max_storage_binding: u64,
    hdr_texels: Option<u64>,
    fluorescent: bool,
) -> Option<&'static str> {
    gpu_ineligibility_reason(spec, max_storage_binding, hdr_texels, fluorescent)
        .or_else(|| (tool_count > GPU_MAX_TOOL_PRIMITIVES).then_some(REASON_TOO_MANY_TOOLS))
}

/// Builds the 16x16 4-spp Spinel self-check scene (section 6.5).
#[must_use]
pub fn self_check_scene() -> SceneSpec {
    let planes = StandardGemCuts::standard_round_brilliant();
    SceneSpec {
        planes: planes_to_data(&planes),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Spinel"),
        camera: CameraSpec {
            yaw: 0.0,
            pitch: 0.5,
            distance: 4.5,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.0,
            0.0,
            Backdrop::AsLit,
            16.0,
        ),
        max_bounces: 8,
        width: 16,
        height: 16,
        hdr_id: None,
    }
}

/// The lifecycle state of a GPU adapter's admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdmissionState {
    /// Never tested.
    #[default]
    Unknown,
    /// A test or compile is running (a record left in this state is a crash marker).
    Probing,
    /// Passed.
    Ok,
    /// Failed, or the GPU was lost.
    Failed,
}

/// The admission record of one adapter, tracking its compile verdict and compile times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdmissionRecord {
    /// The verdict.
    pub state: AdmissionState,
    /// Compile time per kernel in milliseconds.
    pub per_kernel_ms: BTreeMap<String, f64>,
    /// The slowest kernel.
    pub max_ms: f64,
    /// All kernels together.
    pub total_ms: f64,
    /// Why it failed.
    pub reason: Option<String>,
    /// ISO-8601 UTC time of the last change.
    pub when: String,
}

impl Default for AdmissionRecord {
    fn default() -> Self {
        Self {
            state: AdmissionState::Unknown,
            per_kernel_ms: BTreeMap::new(),
            max_ms: 0.0,
            total_ms: 0.0,
            reason: None,
            when: String::new(),
        }
    }
}

/// The first key whose JSON value parses into an [`AdmissionRecord`] in the
/// [`AdmissionState::Probing`] state (only admission keys are looked at).
#[must_use]
pub fn find_probing_key<'a, I>(entries: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    probing_keys(entries).into_iter().next()
}

/// Every admission key in the [`AdmissionState::Probing`] state, in input order.
#[must_use]
pub fn probing_keys<'a, I>(entries: I) -> Vec<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let prefix = format!("{}{}.", storage_keys::PREFIX, storage_keys::ADMISSION_NAME);
    entries
        .into_iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .filter(|(_, value)| {
            serde_json::from_str::<AdmissionRecord>(value)
                .is_ok_and(|record| record.state == AdmissionState::Probing)
        })
        .map(|(key, _)| key.to_string())
        .collect()
}

/// FNV-1a, 64 bit.
#[must_use]
pub fn fnv1a_64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The Chrome (or Chromium, or Edge) major version in a user-agent string, `"0"` when there
/// is none.
#[must_use]
pub fn chrome_major_from_user_agent(user_agent: &str) -> String {
    for prefix in ["Chrome/", "Chromium/", "Edg/"] {
        if let Some(pos) = user_agent.find(prefix) {
            let rest = &user_agent[pos + prefix.len()..];
            let end = rest.find('.').unwrap_or(rest.len());
            let version = &rest[..end];
            if !version.is_empty() && version.chars().all(|c| c.is_ascii_digit()) {
                return version.to_string();
            }
        }
    }
    "0".to_string()
}

/// What the GPU worker reports about the adapter; the admission key is hashed from it.
///
/// On the WebGPU backend wgpu 30 fills only `description` (vendor and device are 0), and
/// Chrome often leaves that empty, so the adapter's limits are part of the key too: two
/// different GPUs rarely share every limit and a description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFacts<'a> {
    /// Vendor (hex or name).
    pub vendor: &'a str,
    /// Architecture.
    pub architecture: &'a str,
    /// Device.
    pub device: &'a str,
    /// Description.
    pub description: &'a str,
    /// Backend name.
    pub backend: &'a str,
    /// `maxStorageBuffersPerShaderStage`.
    pub max_storage_buffers_per_stage: u32,
    /// `maxStorageBufferBindingSize`.
    pub max_storage_binding: u64,
    /// `maxBufferSize`.
    pub max_buffer: u64,
}

/// The text hashed into the adapter key: every field of `facts`, the browser major version
/// and the kernel set hash, `|`-separated.
#[must_use]
pub fn adapter_key_text(
    facts: &AdapterFacts<'_>,
    browser_major: &str,
    kernel_set_hash: impl Display,
) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        facts.vendor,
        facts.architecture,
        facts.device,
        facts.description,
        facts.backend,
        browser_major,
        kernel_set_hash,
        facts.max_storage_buffers_per_stage,
        facts.max_storage_binding,
        facts.max_buffer,
    )
}

/// The storage key of the admission record of this adapter, browser version and kernel set:
/// `indicatrix.admission.<fnv1a-64 hex>.v1`.
#[must_use]
pub fn adapter_key(
    facts: &AdapterFacts<'_>,
    browser_major: &str,
    kernel_set_hash: impl Display,
) -> String {
    let hash = fnv1a_64(&adapter_key_text(facts, browser_major, kernel_set_hash));
    storage_keys::admission_key(&format!("{hash:016x}"))
}

#[cfg(test)]
mod tests;

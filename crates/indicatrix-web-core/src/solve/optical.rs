//! Optical metrics and tilt curves in the solve Worker.
//!
//! # Same numbers as the desktop
//!
//! - **Current-view metrics** ([`run_metrics_with_map`]) are the desktop render loop's
//!   `compute_or_reuse_metrics` for the pose it renders: `evaluate_gem_optical_metrics`
//!   with the traced material (the render-time overrides applied, exactly the material the
//!   scene is built with), the camera yaw/pitch of the frame and its lighting preset and
//!   light yaw/pitch. The metrics are scored under the radiance the frame is lit with
//!   (`metrics_worker.rs`'s `Request::environment`): the loaded HDR panorama when there is
//!   one, else the preset's rig. Here the panorama is the map the Worker holds, named by
//!   [`MetricsParams::hdr_id`]; a Worker that holds no map with that id scores under the
//!   preset and says so ([`ScoredUnder`]). The cache is the shared
//!   `indicatrix::color::metrics::MetricsCache`, which keys on the map's contents, so the
//!   Worker recomputes on exactly the desktop's changes.
//! - **Tilt curves** ([`run_tilt`]) are the desktop Tilt Performance dialog's sweep
//!   (`gui::tilt::tilt_profile`): `indicatrix::color::metrics::evaluate_all_axes_profiles`,
//!   the four `PROFILE_AZIMUTHS_DEG` axes at 181 points each, under the preset's rig at
//!   the current light (the dialog's sweep never uses the HDR map, so neither does this
//!   one) and the resolved material sized by the stone width ([`tilt_material_spec`]; the
//!   desktop sizes it with `material_for_stone`) -- the dialog states that inclusions, c-axis
//!   orientation, edge rounding and the frosted girdle are not applied to it, so the
//!   request's other overrides are dropped here.
//!
//! Neither request carries a design: the caller sends the stone's planes and the settings
//! it renders with (see [`MetricsParams::from_scene`]), and the message's `design_toml` is
//! ignored (send an empty string).

use std::sync::atomic::Ordering;

use indicatrix::{
    color::metrics::{
        AxisProfile, GemOpticalMetrics, MetricsCache, PROFILE_AZIMUTHS_DEG, SweepProgress,
        compute_or_reuse_pose_metrics, evaluate_all_axes_profiles_stepped, total_evaluations,
    },
    geometry::plane::GpuFacetPlane,
    optics::raytracer::{EnvironmentSource, LightingPreset},
    renderer::env_map::EnvironmentMap,
};
use serde::{Deserialize, Serialize};

use super::{SolveHooks, SolveResponse};
use crate::scene::{
    MaterialOverridesSpec, MaterialSpec, PlaneData, SceneSpec, resolve_scene_material,
};

#[cfg(test)]
mod tests;

/// What a current-view metrics job needs: the stone, the traced material and the pose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsParams {
    /// The stone's facet planes.
    pub planes: Vec<PlaneData>,
    /// The material, resolved like the render's (overrides included).
    pub material: MaterialSpec,
    /// Camera yaw in radians.
    pub yaw: f32,
    /// Camera pitch in radians.
    pub pitch: f32,
    /// The lighting preset the metrics are scored under (`LightingPreset::index`) when no
    /// map is held for [`Self::hdr_id`].
    pub preset_index: i32,
    /// Light yaw in radians.
    pub light_yaw: f32,
    /// Light pitch in radians.
    pub light_pitch: f32,
    /// The HDR map the metrics are scored under: `Some(id)` means "under the map the
    /// Worker holds with this id" (the viewport is lit by it); `None` means the preset.
    pub hdr_id: Option<u64>,
}

impl MetricsParams {
    /// The metrics inputs of the frame `spec` renders: its planes, material, camera pose,
    /// lighting preset, light direction and HDR map. Two scenes that differ only in
    /// exposure, size, bounces or finishes give equal params, so those changes recompute
    /// nothing; another map (or none) gives different ones, which is what makes a new
    /// map recompute. The preset and light stay in the params under a map too: they are
    /// what a Worker without that map scores under.
    #[must_use]
    pub fn from_scene(spec: &SceneSpec) -> Self {
        Self {
            planes: spec.planes.clone(),
            material: spec.material.clone(),
            yaw: spec.camera.yaw,
            pitch: spec.camera.pitch,
            preset_index: spec.lighting.preset_index,
            light_yaw: spec.lighting.light_yaw,
            light_pitch: spec.lighting.light_pitch,
            hdr_id: spec.hdr_id,
        }
    }
}

/// What a metrics result was scored under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScoredUnder {
    /// The lighting preset's analytic rig (`LightingPreset::index`), at the job's light
    /// pose.
    Preset(i32),
    /// The HDR map the Worker holds, by the id the page gave it.
    HdrMap(u64),
}

impl ScoredUnder {
    /// The HUD's note on what the numbers were scored under; empty when there is nothing
    /// to add. `asked_map` is whether the request named an HDR map.
    ///
    /// Nothing is said for a request that did not ask for a map: that is the preset the
    /// viewport shows. A request that did and was scored under the map says so; one that
    /// was scored under the preset instead (the Worker holds no such map) says which preset
    /// and that the map is not loaded.
    #[must_use]
    pub fn hud_note(self, asked_map: bool) -> String {
        if !asked_map {
            return String::new();
        }
        match self {
            Self::HdrMap(_) => "Scored under the HDR map".to_string(),
            Self::Preset(index) => format!(
                "Scored under {}, map not loaded",
                LightingPreset::from_index(index).label()
            ),
        }
    }
}

/// What the Tilt dialog says the sweep was scored under.
///
/// Always the preset's rig at the current light (the desktop's sweep never uses the HDR
/// map), with a reminder that a loaded map is not applied when `map_loaded`.
#[must_use]
pub fn tilt_lighting_note(preset_index: i32, map_loaded: bool) -> String {
    let label = LightingPreset::from_index(preset_index).label();
    if map_loaded {
        format!(
            "Scored under the {label} rig at the current light; the loaded HDR map is not \
             used for this sweep, as in the desktop's dialog."
        )
    } else {
        format!("Scored under the {label} rig at the current light.")
    }
}

/// The five pose metrics (`GemOpticalMetrics`) as plain data, and what they were scored
/// under.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricsResultData {
    /// Percentage of incident rays visibly returned to the observer.
    pub brilliance_pct: f32,
    /// Display-scaled energy-weighted F-line/C-line angular separation.
    pub fire_index: f32,
    /// Combined spatial+temporal light-return contrast, 0-100.
    pub scintillation_pct: f32,
    /// Percentage of incident rays that leaked out through the pavilion.
    pub windowing_pct: f32,
    /// Percentage of incident rays trapped, absorbed, or not visibly returned.
    pub extinction_pct: f32,
    /// The environment these were scored under.
    pub scored_under: ScoredUnder,
}

impl MetricsResultData {
    /// `metrics` as plain data, scored under `scored_under`.
    #[must_use]
    pub const fn new(metrics: GemOpticalMetrics, scored_under: ScoredUnder) -> Self {
        Self {
            brilliance_pct: metrics.brilliance_pct,
            fire_index: metrics.fire_index,
            scintillation_pct: metrics.scintillation_pct,
            windowing_pct: metrics.windowing_pct,
            extinction_pct: metrics.extinction_pct,
            scored_under,
        }
    }
}

/// What a tilt-sweep job needs: the stone, the material and the light.
///
/// The camera is not an input (the sweep moves it itself) and the material's render-time
/// overrides are ignored (see the module doc comment), except the stone size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltParams {
    /// The stone's facet planes.
    pub planes: Vec<PlaneData>,
    /// The material; only its name, custom list, linked-design part and
    /// `overrides.stone_width_mm` are used.
    pub material: MaterialSpec,
    /// The lighting preset the sweep is scored under (`LightingPreset::index`).
    pub preset_index: i32,
    /// Light yaw in radians.
    pub light_yaw: f32,
    /// Light pitch in radians.
    pub light_pitch: f32,
}

impl TiltParams {
    /// The sweep inputs of the frame `spec` renders. The material's render-time overrides are
    /// left at their defaults, since the sweep ignores them, except the stone size
    /// ([`tilt_material_spec`]): two scenes that differ only in the inclusions, axis or edge
    /// rounding give equal params, so changing those does not stale the curves, while a new
    /// stone width (which changes the absorption the sweep scores) does.
    #[must_use]
    pub fn from_scene(spec: &SceneSpec) -> Self {
        Self {
            planes: spec.planes.clone(),
            material: tilt_material_spec(&spec.material),
            preset_index: spec.lighting.preset_index,
            light_yaw: spec.lighting.light_yaw,
            light_pitch: spec.lighting.light_pitch,
        }
    }
}

/// The analytic rig a metrics or tilt job is scored under: the preset at the job's light
/// pose. An out-of-range preset index falls back to the default preset, like every other
/// consumer of the index.
const fn job_environment(
    preset_index: i32,
    light_yaw: f32,
    light_pitch: f32,
) -> EnvironmentSource<'static> {
    LightingPreset::from_index(preset_index).studio(1.0, light_yaw, light_pitch)
}

/// One axis of a finished sweep: three 181-point curves (`TILT_ANGLES_DEG`-indexed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltAxisData {
    /// The axis' azimuth in degrees (`PROFILE_AZIMUTHS_DEG`).
    pub azimuth_deg: f32,
    /// Brilliance %, 181 points.
    pub brilliance: Vec<f32>,
    /// Extinction %, 181 points.
    pub extinction: Vec<f32>,
    /// Windowing %, 181 points.
    pub windowing: Vec<f32>,
}

impl TiltAxisData {
    fn new(azimuth_deg: f32, profile: &AxisProfile) -> Self {
        Self {
            azimuth_deg,
            brilliance: profile.brilliance.to_vec(),
            extinction: profile.extinction.to_vec(),
            windowing: profile.windowing.to_vec(),
        }
    }

    /// The curves as fixed-size arrays; `None` unless each has exactly 181 points.
    #[must_use]
    pub fn to_profile(&self) -> Option<AxisProfile> {
        Some(AxisProfile {
            brilliance: self.brilliance.as_slice().try_into().ok()?,
            extinction: self.extinction.as_slice().try_into().ok()?,
            windowing: self.windowing.as_slice().try_into().ok()?,
        })
    }
}

/// A finished tilt sweep: one entry per `PROFILE_AZIMUTHS_DEG` axis, in that order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltResultData {
    /// The four axes.
    pub axes: Vec<TiltAxisData>,
}

const fn failed(message: String) -> SolveResponse {
    SolveResponse::AnalysisFailed {
        message,
        missing_anchor: false,
    }
}

/// The planes as the tracer's type, or why there is no stone.
fn stone_planes(data: &[PlaneData]) -> Result<Vec<GpuFacetPlane>, String> {
    if data.is_empty() {
        return Err("the design has no facet planes to measure".to_string());
    }
    Ok(data.iter().copied().map(GpuFacetPlane::from).collect())
}

/// Computes the current-view metrics under the preset's rig, holding no HDR map -- see
/// [`run_metrics_with_map`].
#[must_use]
pub fn run_metrics(cache: &mut Option<MetricsCache>, params: &MetricsParams) -> SolveResponse {
    run_metrics_with_map(cache, params, None)
}

/// Computes the current-view metrics -- see the module doc comment. `cache` is the
/// Worker's `MetricsCache`; a pose it already holds is answered without evaluating.
///
/// `held` is the Worker's decoded HDR map and its id. The metrics are scored under it when
/// `params.hdr_id` names that id; otherwise (no map requested, none held, or another one)
/// under the preset's rig, and the result says which.
#[must_use]
pub fn run_metrics_with_map(
    cache: &mut Option<MetricsCache>,
    params: &MetricsParams,
    held: Option<(u64, &EnvironmentMap)>,
) -> SolveResponse {
    let planes = match stone_planes(&params.planes) {
        Ok(planes) => planes,
        Err(message) => return failed(message),
    };
    let material = match resolve_scene_material(&params.material, &planes) {
        Ok(material) => material,
        Err(error) => return failed(error.to_string()),
    };
    let (environment, scored_under) = match (params.hdr_id, held) {
        (Some(wanted), Some((id, map))) if wanted == id => {
            (EnvironmentSource::HdrMap(map), ScoredUnder::HdrMap(id))
        }
        _ => (
            job_environment(params.preset_index, params.light_yaw, params.light_pitch),
            ScoredUnder::Preset(params.preset_index),
        ),
    };
    let metrics = compute_or_reuse_pose_metrics(
        cache,
        &planes,
        &material,
        params.yaw,
        params.pitch,
        environment,
    );
    SolveResponse::Metrics(MetricsResultData::new(metrics, scored_under))
}

/// The material the tilt sweep traces: the resolved material with no render-time override
/// (inclusions, axis, edge rounding) except the stone size, which the render's own scene
/// carries in `overrides.stone_width_mm` (the linked design's girdle diameter, or the
/// stone-width control). The desktop's tilt profile sizes its material the same way
/// (`material_for_stone(material, ctx.stone_width_mm, ..)`), so web tilt == desktop tilt.
#[must_use]
pub fn tilt_material_spec(material: &MaterialSpec) -> MaterialSpec {
    MaterialSpec {
        overrides: MaterialOverridesSpec {
            stone_width_mm: material.overrides.stone_width_mm,
            ..MaterialOverridesSpec::default()
        },
        ..material.clone()
    }
}

/// Runs the four-axis tilt sweep -- see the module doc comment.
///
/// `hooks.on_sweep` is called before every evaluation, and a set `hooks.cancel` (checked
/// right after) ends the sweep with [`SolveResponse::Cancelled`].
#[must_use]
pub fn run_tilt(params: &TiltParams, hooks: &SolveHooks<'_>) -> SolveResponse {
    let planes = match stone_planes(&params.planes) {
        Ok(planes) => planes,
        Err(message) => return failed(message),
    };
    let bare = tilt_material_spec(&params.material);
    let material = match resolve_scene_material(&bare, &planes) {
        Ok(material) => material,
        Err(error) => return failed(error.to_string()),
    };
    let swept = evaluate_all_axes_profiles_stepped(
        &planes,
        &material,
        job_environment(params.preset_index, params.light_yaw, params.light_pitch),
        &mut |progress| {
            (hooks.on_sweep)(progress);
            !hooks.cancel.load(Ordering::Relaxed)
        },
    );
    swept.map_or(SolveResponse::Cancelled, |axes| {
        SolveResponse::TiltCurves(TiltResultData {
            axes: PROFILE_AZIMUTHS_DEG
                .iter()
                .zip(&axes)
                .map(|(&azimuth, profile)| TiltAxisData::new(azimuth, profile))
                .collect(),
        })
    })
}

/// The stage line of a running sweep: `Tilt sweep: axis 2 of 4 (45°) -- 300 of 724 points`.
#[must_use]
pub fn sweep_status(progress: SweepProgress) -> String {
    let azimuth = PROFILE_AZIMUTHS_DEG
        .get(progress.axis)
        .copied()
        .unwrap_or_default();
    format!(
        "Tilt sweep: axis {} of {} ({azimuth:.0}\u{b0}) \u{2014} {} of {} points",
        progress.axis + 1,
        PROFILE_AZIMUTHS_DEG.len(),
        progress.done,
        progress.total
    )
}

/// The completion fraction of a running sweep.
#[must_use]
pub fn sweep_fraction(progress: SweepProgress) -> f32 {
    (progress.done as f32 / total_evaluations().max(1) as f32).clamp(0.0, 1.0)
}

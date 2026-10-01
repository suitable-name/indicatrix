//! The browser app's render settings, their session payload and their render scene.
//!
//! [`RenderSettings`] are the render and view settings, [`SessionPayload`] the
//! tab-session payload they persist in, and [`scene_spec`] turns them into a render
//! scene the way the desktop turns its settings into its render context.
//!
//! # Persistence
//!
//! The page stores a [`SessionPayload`] as JSON under `sessionStorage`'s
//! `indicatrix.settings.v1` ([`SessionPayload::to_json`] / [`SessionPayload::from_json`]).
//! Every field is individually optional when parsed (`#[serde(default)]`), so a payload
//! from an older build still loads, and the keys the first web build wrote are kept
//! unchanged. Materials and lighting are stored by NAME, like the desktop's settings
//! file, so a reordered list never changes what a stored setting means.
//!
//! # Desktop defaults and conversions
//!
//! Defaults are the desktop's (`apps/indicatrix-cut/src/settings/model/app_settings.rs`):
//! Diamond, "Light tent + black cards", exposure 1.0, light 48 deg / 54 deg, grey
//! backdrop, 12 bounces, 256 live samples, camera 0.60 / 0.45 / 2.4, denoise on,
//! material linked to the design, every material override off. [`scene_spec`] applies
//! the desktop's own conversions (`gui::startup_settings::apply_loaded_settings`, the
//! light and material callbacks, `gui::editor::view::inspector::
//! sync_viewport_material_link`): light yaw in radians, light pitch in radians clamped to
//! `0.15..=1.55`, the c-axis from tilt/azimuth, and -- while linked -- the design's
//! traced material, its RI/body-colour overrides and its girdle diameter as the stone
//! width.

use glam::Vec3;
use indicatrix::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{DEFAULT_MAX_BOUNCES, DEFAULT_POSE, LightingPreset},
    },
    render_setup::Backdrop,
};
use indicatrix_cut_core::Design;
use indicatrix_editor::material_lookup::traced_material_for;
use serde::{Deserialize, Serialize};

use crate::{
    display::{DEFAULT_EXPORT_EDGE, MAX_EXPORT_EDGE},
    render::{DEFAULT_LIVE_SPP, EXPORT_MAX_SPP, EXPORT_MIN_SPP, LIVE_MAX_SPP, LIVE_MIN_SPP},
    scene::{
        CameraSpec, DesignMaterialOverrides, FinishSpec, LightingSpec, MaterialOverridesSpec,
        MaterialSpec, SceneSpec, planes_to_data,
    },
};

#[cfg(test)]
mod tests;

/// The live sample target range on the web.
pub const LIVE_SPP_RANGE: (u32, u32) = (LIVE_MIN_SPP, LIVE_MAX_SPP);
/// The bounce range on the web.
pub const BOUNCE_RANGE: (u32, u32) = (4, 24);
/// The desktop's exposure clamp (`apply_loaded_settings`: `0.2..=5.0`).
pub const EXPOSURE_RANGE: (f32, f32) = (0.2, 5.0);
/// The desktop's inclusion-scattering clamp (`0.0..=3.0`).
pub const INCLUSION_RANGE: (f32, f32) = (0.0, 3.0);
/// The desktop's edge-rounding slider range (`0.0..=0.03`).
pub const EDGE_ROUNDING_RANGE: (f32, f32) = (0.0, 0.03);
/// The desktop's stone-size slider range, millimetres (`0.0..=20.0`).
pub const STONE_WIDTH_RANGE: (f32, f32) = (0.0, 20.0);
/// The desktop's light-pitch clamp in radians (`to_radians().clamp(0.15, 1.55)`).
pub const LIGHT_PITCH_RADIANS: (f32, f32) = (0.15, 1.55);

/// The PNG export dialog's last choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// Long edge in pixels (`1..=4096`).
    pub long_edge: u32,
    /// Samples per pixel (`16..=4096`).
    pub spp: u32,
    /// `display::EXPORT_COLOR_SPACES` index: 0 sRGB, 1 Display P3.
    pub color_space: i32,
    /// Denoise before tone mapping (the desktop's export never does, so off).
    pub denoise: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            long_edge: DEFAULT_EXPORT_EDGE,
            spp: 256,
            color_space: 0,
            denoise: false,
        }
    }
}

/// The render and view settings. See the module doc comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    /// The render material's name (a built-in, or a restored custom material); used
    /// while [`Self::link_material`] is off.
    pub material: String,
    /// The lighting preset's label (`LightingPreset::label`).
    pub lighting: String,
    /// Exposure multiplier.
    pub exposure: f32,
    /// Key-light yaw, degrees.
    pub light_yaw_deg: f32,
    /// Key-light pitch, degrees.
    pub light_pitch_deg: f32,
    /// `indicatrix::render_setup::Backdrop::index`.
    pub backdrop_index: i32,
    /// Maximum path bounces.
    pub max_bounces: u32,
    /// Live target samples per pixel.
    pub target_spp: u32,
    /// Camera yaw, radians.
    pub camera_yaw: f32,
    /// Camera pitch, radians.
    pub camera_pitch: f32,
    /// Camera distance.
    pub camera_distance: f32,
    /// The view tab: 0 Render, 1 Solid, 2 Diagram.
    pub view_tab: i32,
    /// The desktop's "Linked to design": the render uses the design's own material
    /// (with its RI and body-colour overrides) and its girdle diameter as stone width.
    pub link_material: bool,
    /// Denoise the settled live image (the desktop's `denoise_enabled`).
    pub denoise: bool,
    /// Frosted girdle.
    pub frosted_girdle: bool,
    /// Crystal-axis override on.
    pub c_axis_override: bool,
    /// Crystal-axis tilt from the table normal, degrees (`0..=90`).
    pub c_axis_tilt_deg: f32,
    /// Crystal-axis azimuth, degrees (`0..=360`).
    pub c_axis_azimuth_deg: f32,
    /// Inclusion scattering `sigma_s` (`0` is off).
    pub inclusion_sigma_s: f32,
    /// Facet edge rounding radius (`0` is off).
    pub edge_rounding_radius: f32,
    /// Stone width in millimetres for the absorption scale (`0` is off); used while
    /// [`Self::link_material`] is off.
    pub stone_width_mm: f32,
    /// Light the stone with the uploaded HDR map, when one is loaded.
    pub use_hdr: bool,
    /// The PNG export dialog.
    pub export: ExportSettings,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            material: "Diamond".to_string(),
            lighting: LightingPreset::LightTent.label().to_string(),
            exposure: 1.0,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            backdrop_index: Backdrop::Grey.index(),
            max_bounces: DEFAULT_MAX_BOUNCES,
            target_spp: DEFAULT_LIVE_SPP,
            camera_yaw: DEFAULT_POSE.yaw,
            camera_pitch: DEFAULT_POSE.pitch,
            camera_distance: DEFAULT_POSE.distance,
            view_tab: 0,
            link_material: true,
            denoise: true,
            frosted_girdle: false,
            c_axis_override: false,
            c_axis_tilt_deg: 0.0,
            c_axis_azimuth_deg: 0.0,
            inclusion_sigma_s: 0.0,
            edge_rounding_radius: 0.0,
            stone_width_mm: 0.0,
            use_hdr: true,
            export: ExportSettings::default(),
        }
    }
}

/// `value` when finite, else `fallback`.
const fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

impl RenderSettings {
    /// Clamps every numeric field into its supported range and replaces a non-finite
    /// float with its default -- a hand-edited or stale payload must never reach the
    /// renderer out of range. Unknown lighting labels map like the desktop's
    /// `LightingPreset::from_label`.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.exposure =
            finite_or(self.exposure, d.exposure).clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1);
        self.light_yaw_deg = finite_or(self.light_yaw_deg, d.light_yaw_deg);
        self.light_pitch_deg =
            finite_or(self.light_pitch_deg, d.light_pitch_deg).clamp(-90.0, 90.0);
        self.camera_yaw = finite_or(self.camera_yaw, d.camera_yaw);
        self.camera_pitch = finite_or(self.camera_pitch, d.camera_pitch).clamp(-1.48, 1.48);
        self.camera_distance = finite_or(self.camera_distance, d.camera_distance).clamp(1.0, 20.0);
        self.max_bounces = self.max_bounces.clamp(BOUNCE_RANGE.0, BOUNCE_RANGE.1);
        self.target_spp = self.target_spp.clamp(LIVE_SPP_RANGE.0, LIVE_SPP_RANGE.1);
        self.view_tab = self.view_tab.clamp(0, 2);
        self.backdrop_index = Backdrop::from_index(self.backdrop_index).index();
        self.lighting = LightingPreset::from_label(&self.lighting)
            .label()
            .to_string();
        self.c_axis_tilt_deg = finite_or(self.c_axis_tilt_deg, 0.0).clamp(0.0, 90.0);
        self.c_axis_azimuth_deg = finite_or(self.c_axis_azimuth_deg, 0.0).clamp(0.0, 360.0);
        self.inclusion_sigma_s =
            finite_or(self.inclusion_sigma_s, 0.0).clamp(INCLUSION_RANGE.0, INCLUSION_RANGE.1);
        self.edge_rounding_radius = finite_or(self.edge_rounding_radius, 0.0)
            .clamp(EDGE_ROUNDING_RANGE.0, EDGE_ROUNDING_RANGE.1);
        self.stone_width_mm =
            finite_or(self.stone_width_mm, 0.0).clamp(STONE_WIDTH_RANGE.0, STONE_WIDTH_RANGE.1);
        self.export.long_edge = self.export.long_edge.clamp(1, MAX_EXPORT_EDGE);
        self.export.spp = self.export.spp.clamp(EXPORT_MIN_SPP, EXPORT_MAX_SPP);
        self.export.color_space = self.export.color_space.clamp(0, 1);
        self
    }

    /// The lighting preset these settings name.
    #[must_use]
    pub fn lighting_preset(&self) -> LightingPreset {
        LightingPreset::from_label(&self.lighting)
    }

    /// The backdrop.
    #[must_use]
    pub const fn backdrop(&self) -> Backdrop {
        Backdrop::from_index(self.backdrop_index)
    }

    /// Whether the lighting preset reads the light direction (every preset except
    /// the ISO hemisphere, whose uniform dome has no key light).
    #[must_use]
    pub fn uses_light_direction(&self) -> bool {
        self.lighting_preset() != LightingPreset::IsoHemisphere
    }

    /// The studio lighting with the desktop's conversions: yaw in radians, pitch in
    /// radians clamped to [`LIGHT_PITCH_RADIANS`].
    #[must_use]
    pub fn lighting_spec(&self) -> LightingSpec {
        LightingSpec::new(
            self.lighting_preset(),
            self.exposure,
            self.light_yaw_deg.to_radians(),
            self.light_pitch_deg
                .to_radians()
                .clamp(LIGHT_PITCH_RADIANS.0, LIGHT_PITCH_RADIANS.1),
            self.backdrop(),
        )
    }

    /// The camera these settings hold.
    #[must_use]
    pub const fn camera_spec(&self) -> CameraSpec {
        CameraSpec {
            yaw: self.camera_yaw,
            pitch: self.camera_pitch,
            distance: self.camera_distance,
        }
    }

    /// The render-time material controls; `stone_width_mm` is the resolved one
    /// ([`RenderMaterial::stone_width_mm`]).
    #[must_use]
    pub fn material_overrides(&self, stone_width_mm: f32) -> MaterialOverridesSpec {
        MaterialOverridesSpec {
            inclusion_sigma_s: self.inclusion_sigma_s,
            c_axis_override: self.c_axis_override.then(|| {
                c_axis_from_angles(self.c_axis_tilt_deg, self.c_axis_azimuth_deg).to_array()
            }),
            edge_rounding_radius: self.edge_rounding_radius,
            stone_width_mm,
        }
    }

    /// The facet finishes.
    #[must_use]
    pub const fn finish_spec(&self) -> FinishSpec {
        if self.frosted_girdle {
            FinishSpec::FrostedGirdle
        } else {
            FinishSpec::AllPolished
        }
    }
}

/// The desktop's `gui::optics::c_axis::angles_to_c_axis`: tilt from +Y (the table
/// normal) towards the azimuth direction in the XZ plane.
#[must_use]
pub fn c_axis_from_angles(tilt_deg: f32, azimuth_deg: f32) -> Vec3 {
    let (sin_theta, cos_theta) = tilt_deg.to_radians().sin_cos();
    let (sin_phi, cos_phi) = azimuth_deg.to_radians().sin_cos();
    Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
}

/// The material a render traces, decided like the desktop's viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderMaterial {
    /// The traced material name.
    pub name: String,
    /// The design's overrides when linked.
    pub linked_design: Option<DesignMaterialOverrides>,
    /// The stone width for the absorption scale (the design's girdle diameter when
    /// linked).
    pub stone_width_mm: f32,
}

/// The render material for `settings` and the loaded `design`.
///
/// While linked, the desktop's `sync_viewport_material_link` (the design's traced name from
/// `traced_material_for`, its RI/body-colour overrides and its girdle diameter);
/// otherwise the header's material and the stone-width control.
///
/// # Errors
///
/// `traced_material_for`'s reason when a linked design's material resolves to
/// nothing (the desktop suspends its render then).
pub fn render_material(
    settings: &RenderSettings,
    design: Option<&Design>,
    custom_materials: &[GemMaterial],
) -> Result<RenderMaterial, String> {
    match design {
        Some(design) if settings.link_material => {
            let (name, unresolved) = traced_material_for(design, custom_materials);
            if let Some(reason) = unresolved {
                return Err(reason);
            }
            Ok(RenderMaterial {
                name,
                linked_design: Some(DesignMaterialOverrides::from_selection(&design.material)),
                stone_width_mm: design.girdle_diameter_mm.unwrap_or(0.0) as f32,
            })
        }
        _ => Ok(RenderMaterial {
            name: settings.material.clone(),
            linked_design: None,
            stone_width_mm: settings.stone_width_mm,
        }),
    }
}

/// Everything besides the settings that a render scene needs.
pub struct SceneInputs<'a> {
    /// The stone's planes (the solved design's).
    pub planes: &'a [GpuFacetPlane],
    /// From [`render_material`].
    pub material: &'a RenderMaterial,
    /// The session's custom materials.
    pub custom_materials: &'a [GemMaterial],
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// The loaded HDR map's id when it lights this scene.
    pub hdr_id: Option<u64>,
}

/// The render scene for `settings` -- see the module doc comment.
#[must_use]
pub fn scene_spec(settings: &RenderSettings, inputs: &SceneInputs<'_>) -> SceneSpec {
    SceneSpec {
        planes: planes_to_data(inputs.planes),
        finishes: settings.finish_spec(),
        material: MaterialSpec {
            name: inputs.material.name.clone(),
            custom_materials: inputs.custom_materials.to_vec(),
            linked_design: inputs.material.linked_design,
            overrides: settings.material_overrides(inputs.material.stone_width_mm),
        },
        camera: settings.camera_spec(),
        lighting: settings.lighting_spec(),
        max_bounces: settings.max_bounces,
        width: inputs.width,
        height: inputs.height,
        hdr_id: inputs.hdr_id,
    }
}

/// The desktop's default auto-solve budget in milliseconds
/// (`DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS`).
pub const DEFAULT_AUTO_SOLVE_BUDGET_MS: u32 = 300;

/// The five budgets the dock's auto-solve combo offers, in combo order: `0` is "Off", then
/// 150 ms, 300 ms, 1 s and 3 s (the desktop's `editor_command_bar.slint`).
pub const AUTO_SOLVE_BUDGETS_MS: [u32; 5] = [0, 150, 300, 1000, 3000];

/// The largest auto-solve budget a stored payload may carry.
const AUTO_SOLVE_BUDGET_CAP_MS: u32 = 60_000;

/// The combo index of `budget_ms`: its position in [`AUTO_SOLVE_BUDGETS_MS`], or the
/// last entry (3 s) for a budget that is not one of the five -- the desktop's rule.
#[must_use]
pub fn auto_solve_budget_index(budget_ms: u32) -> usize {
    AUTO_SOLVE_BUDGETS_MS
        .iter()
        .position(|&b| b == budget_ms)
        .unwrap_or(AUTO_SOLVE_BUDGETS_MS.len() - 1)
}

/// What `sessionStorage["indicatrix.settings.v1"]` holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionPayload {
    /// The render and view settings.
    pub render: RenderSettings,
    /// The design's recorded `.asc` name (`None` for a design never saved).
    pub design_name: Option<String>,
    /// Whether the design had unsaved changes.
    pub design_unsaved: bool,
    /// The dock's auto-solve budget in milliseconds, `0` for off
    /// ([`AUTO_SOLVE_BUDGETS_MS`]). A payload from an older build has none and takes
    /// [`DEFAULT_AUTO_SOLVE_BUDGET_MS`].
    pub auto_solve_budget_ms: u32,
}

impl Default for SessionPayload {
    fn default() -> Self {
        Self {
            render: RenderSettings::default(),
            design_name: None,
            design_unsaved: false,
            auto_solve_budget_ms: DEFAULT_AUTO_SOLVE_BUDGET_MS,
        }
    }
}

impl SessionPayload {
    /// The stored auto-solve budget, capped so a hand-edited payload cannot make the
    /// dock's threshold absurd.
    #[must_use]
    pub fn auto_solve_budget(&self) -> u32 {
        self.auto_solve_budget_ms.min(AUTO_SOLVE_BUDGET_CAP_MS)
    }

    /// Parses a stored payload (unsanitised; see [`RenderSettings::sanitized`]).
    ///
    /// # Errors
    ///
    /// The JSON parser's message.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// The JSON to store.
    ///
    /// # Errors
    ///
    /// The serializer's message (in practice unreachable for these types).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
}

/// The render material combo's entries: every built-in, sorted case-insensitively
/// (the desktop's `built_in_material_option_names`), then every custom material
/// whose name is not already listed.
#[must_use]
pub fn material_options(custom: &[GemMaterial]) -> Vec<String> {
    let mut names: Vec<String> = GemMaterial::all_materials()
        .into_iter()
        .map(|m| m.name)
        .collect();
    names.sort_by_key(|a| a.to_ascii_lowercase());
    for material in custom {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&material.name)) {
            names.push(material.name.clone());
        }
    }
    names
}

/// The lighting combo's entries, in `LightingPreset::ALL` (index) order.
#[must_use]
pub fn lighting_options() -> Vec<String> {
    LightingPreset::ALL
        .iter()
        .map(|preset| preset.label().to_string())
        .collect()
}

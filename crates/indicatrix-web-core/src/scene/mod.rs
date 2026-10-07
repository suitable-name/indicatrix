//! [`SceneSpec`]: a render scene as plain serde data, and [`OwnedScene`], which builds
//! from it exactly the scene the desktop builds for the same settings.
//!
//! # Same scene as the desktop
//!
//! The desktop's live render loop (`apps/indicatrix-cut/src/bridge/render_thread`)
//! assembles a frame from its settings in this order, and [`OwnedScene::build`] does the
//! same with the same public `indicatrix` calls:
//!
//! 1. **Material.** The catalogue is `GemMaterial::all_materials()` plus the session's
//!    custom materials. The linked design's override is
//!    `indicatrix_editor::material_lookup::traced_gem_material` (the RI override and body
//!    color applied through `MaterialSelection::apply_overrides`). Then
//!    `render_setup::resolve_material_with_override` and
//!    `render_setup::apply_material_overrides`, the stone width measured by
//!    `render_setup::measure_model_width` only when the stone-size control is on.
//! 2. **Finishes.** All polished, or `geometry::girdle::girdle_facet_finishes` for the
//!    frosted-girdle toggle.
//! 3. **Camera.** `Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG)`.
//! 4. **Environment.** An HDR map when one is loaded, else
//!    `preset.studio(exposure, light_yaw, light_pitch).with_backdrop(backdrop.level())`.
//!
//! The one deliberate difference: an unresolvable material is a [`SceneError`] here,
//! where the desktop suspends its render loop on the same condition
//! (`SuspensionFlags::material_unresolved`) and never reaches its diamond fallback.

use std::{fmt, sync::Arc};

use indicatrix::{
    geometry::{GpuFacetPlane, girdle::girdle_facet_finishes},
    optics::{
        chromophore::ChromophoreCatalogue,
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish, LightingPreset,
            build_plane_soa,
        },
    },
    render_setup::{
        Backdrop, MaterialOverrides, apply_material_overrides_for_mode, measure_model_width,
        resolve_material_with_override,
    },
    renderer::{env_map::EnvironmentMap, frame_scene::FrameScene},
    simd::PlanesSoA32,
};
use indicatrix_cut_core::{
    MaterialSelection,
    native::{SnapshotColor, gem_material_from_custom_snapshot, snapshot_color},
};
use indicatrix_editor::material_lookup::{EditorMaterialLookup, traced_gem_material};
use indicatrix_formats::native::{ColorRecipeDto, CustomMaterialSnapshot};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// The largest frame, in pixels, a scene may ask for: a 4096 x 4096 export (the PNG
/// export caps the long edge at 4096 px).
pub const MAX_FRAME_PIXELS: u64 = 4096 * 4096;

/// One facet plane as plain data: exactly `GpuFacetPlane`'s two fields (`n . x + d <= 0`
/// is inside the stone).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlaneData {
    /// Outward unit normal.
    pub normal: [f32; 3],
    /// Plane offset.
    pub d: f32,
}

impl From<GpuFacetPlane> for PlaneData {
    fn from(plane: GpuFacetPlane) -> Self {
        Self {
            normal: plane.normal,
            d: plane.d,
        }
    }
}

impl From<PlaneData> for GpuFacetPlane {
    /// Copies the fields verbatim (no renormalisation), so a plane survives the round
    /// trip bit for bit.
    fn from(plane: PlaneData) -> Self {
        Self {
            normal: plane.normal,
            d: plane.d,
        }
    }
}

/// Converts a plane list to its plain-data form.
#[must_use]
pub fn planes_to_data(planes: &[GpuFacetPlane]) -> Vec<PlaneData> {
    planes.iter().copied().map(PlaneData::from).collect()
}

/// Per-facet surface finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FinishSpec {
    /// Every facet polished (the desktop's frosted-girdle toggle off).
    #[default]
    AllPolished,
    /// The girdle band frosted, classified by `girdle_facet_finishes` exactly like the
    /// desktop's `GirdleFinishCache` (the toggle on).
    FrostedGirdle,
    /// An explicit per-plane list, `true` = frosted, in plane order. A shorter list is
    /// padded with polished facets, like `FrameScene::facet_finishes`.
    PerFacet(Vec<bool>),
}

/// A custom material as the design file's `[material.custom]` table records it --
/// the fields `native::gem_material_from_custom_snapshot` reads.
///
/// A mirror rather than `CustomMaterialSnapshot` itself: that type flattens unknown TOML
/// keys into a map, which postcard (not self-describing) cannot encode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMaterialSpec {
    /// The name the material is registered and looked up under.
    pub name: String,
    /// Mean refractive index.
    pub mean_ri: f64,
    /// Dispersion (`n_F - n_C`-style delta).
    pub dispersion_delta: f64,
    /// Birefringence delta.
    pub birefringence_delta: f64,
    /// The body color (the snapshot's `absorption_rgb`); `None` is colorless.
    pub absorption_rgb: Option<[f64; 3]>,
    /// The snapshot's physics color recipe (`color_recipe`); when present the material
    /// renders from its stored resolved bands, `absorption_rgb` being only the fallback.
    pub color_recipe: Option<ColorRecipeDto>,
    /// The snapshot's seven-band body colour (`absorption_bands_per_mm`, rows
    /// `[centre_nm, width_nm, amplitude_per_mm]`); empty is "no bands" and `absorption_rgb`
    /// colours the material. Wire version 12.
    #[serde(default)]
    pub absorption_bands: Vec<[f64; 3]>,
}

impl CustomMaterialSpec {
    /// Copies the fields a snapshot's material is built from.
    #[must_use]
    pub fn from_snapshot(name: impl Into<String>, snapshot: &CustomMaterialSnapshot) -> Self {
        Self {
            name: name.into(),
            mean_ri: snapshot.mean_ri,
            dispersion_delta: snapshot.dispersion_delta,
            birefringence_delta: snapshot.birefringence_delta,
            absorption_rgb: snapshot.absorption_rgb,
            color_recipe: snapshot.color_recipe.clone(),
            absorption_bands: snapshot.absorption_bands_per_mm.clone().unwrap_or_default(),
        }
    }

    fn to_snapshot(&self) -> CustomMaterialSnapshot {
        let rows: Vec<[f32; 3]> = self
            .absorption_bands
            .iter()
            .map(|row| row.map(|v| v as f32))
            .collect();
        CustomMaterialSnapshot::new(
            self.mean_ri,
            self.dispersion_delta,
            self.birefringence_delta,
            None,
            "",
            "",
        )
        .with_absorption_rgb(self.absorption_rgb)
        .with_color_recipe(self.color_recipe.clone())
        .with_absorption_bands(&rows)
    }

    /// Builds the material through `native::gem_material_from_custom_snapshot`, the
    /// same conversion the desktop uses when it restores a file's custom material.
    #[must_use]
    pub fn to_gem_material(&self) -> GemMaterial {
        gem_material_from_custom_snapshot(&self.name, &self.to_snapshot())
    }

    /// Whether this material's color renders from its physics recipe: the explicit flag
    /// [`resolve_scene_material_with`] takes (the 7 mm default stone width applies to such
    /// a material even when its recipe emits no bands, e.g. a pure host). Derived from the
    /// recipe the spec carries -- present, physics active, and the top-level color not
    /// edited by an older build (`native::snapshot_color`) -- never from the shape of the
    /// built material's bands.
    #[must_use]
    pub fn is_physics_color(&self) -> bool {
        self.color_recipe.is_some()
            && matches!(
                snapshot_color(&self.to_snapshot()),
                SnapshotColor::Physics(_)
            )
    }

    /// The fluorescence this material renders with: the emitters of its physics recipe
    /// (`ColorMode::fluorescence`, resolved from the recipe's elements and concentrations
    /// against the catalogue), empty for every other material -- fantasy colors, edited
    /// ones ([`Self::is_physics_color`] false) and recipes without emitters.
    #[must_use]
    pub fn fluorescence(&self) -> Fluorescence {
        match snapshot_color(&self.to_snapshot()) {
            SnapshotColor::Physics(mode) => mode.fluorescence(ChromophoreCatalogue::global()),
            _ => Fluorescence::new(Vec::new()),
        }
    }
}

/// The linked design's per-design material overrides (`MaterialSelection`'s RI
/// override and body color).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DesignMaterialOverrides {
    /// `MaterialSelection::refractive_index_override`: a flat dispersion at this `n_d`.
    pub refractive_index_override: Option<f64>,
    /// `MaterialSelection::body_color_override`: an isotropic body color.
    pub body_color_override: Option<[f32; 3]>,
    /// `MaterialSelection::body_color_bands_override`: the seven-band body colour of the
    /// path-aware L*C*h editor, rows `[centre_nm, width_nm, amplitude_per_mm]`. Empty is
    /// "no bands" (the triple above colours the stone); non-empty bands win over the triple.
    /// Wire version 12.
    #[serde(default)]
    pub body_color_bands: Vec<[f32; 3]>,
    /// `MaterialSelection::absorption_path_scale_override`: mm per model unit for the bands.
    /// Wire version 12.
    #[serde(default)]
    pub absorption_path_scale_override: Option<f32>,
}

impl DesignMaterialOverrides {
    /// Copies the render-relevant overrides from a design's selection.
    #[must_use]
    pub fn from_selection(selection: &MaterialSelection) -> Self {
        Self {
            refractive_index_override: selection.refractive_index_override,
            body_color_override: selection.body_color_override,
            body_color_bands: selection
                .body_color_bands_override
                .clone()
                .unwrap_or_default(),
            absorption_path_scale_override: selection.absorption_path_scale_override,
        }
    }

    fn to_selection(&self) -> MaterialSelection {
        let bands = (!self.body_color_bands.is_empty()).then(|| self.body_color_bands.clone());
        MaterialSelection {
            refractive_index_override: self.refractive_index_override,
            body_color_override: self.body_color_override,
            absorption_path_scale_override: bands.as_ref().and(self.absorption_path_scale_override),
            body_color_bands_override: bands,
            ..MaterialSelection::default()
        }
    }
}

/// The render-time material controls, field for field `render_setup::MaterialOverrides`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct MaterialOverridesSpec {
    /// Inclusion scattering `sigma_s`; `0.0` is off.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis override; `None` is "as cut".
    pub c_axis_override: Option<[f32; 3]>,
    /// Facet edge rounding radius; `0.0` is off.
    pub edge_rounding_radius: f32,
    /// Girdle width in millimetres for the absorption scale; `0.0` is off.
    pub stone_width_mm: f32,
}

impl MaterialOverridesSpec {
    /// The `render_setup::MaterialOverrides` this describes.
    #[must_use]
    pub fn to_overrides(self) -> MaterialOverrides {
        MaterialOverrides {
            inclusion_sigma_s: self.inclusion_sigma_s,
            c_axis_override: self.c_axis_override.map(glam::Vec3::from_array),
            edge_rounding_radius: self.edge_rounding_radius,
            stone_width_mm: self.stone_width_mm,
        }
    }
}

/// Which material to trace, resolved the way the desktop resolves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialSpec {
    /// The traced material name (the desktop's `RenderContext::material_name`): a
    /// catalogue name like `"Sapphire"`, or a custom material's name.
    pub name: String,
    /// The session's custom materials (the desktop's `custom_materials`), as built
    /// materials; they win over a built-in with the same name. A file's custom-material
    /// snapshot becomes one through [`CustomMaterialSpec::to_gem_material`].
    pub custom_materials: Vec<GemMaterial>,
    /// `Some` when the render is linked to the editor's design (the desktop's
    /// `material_override`): the material is then `traced_gem_material(name, ..)` with
    /// these overrides applied.
    pub linked_design: Option<DesignMaterialOverrides>,
    /// The render-time material controls.
    pub overrides: MaterialOverridesSpec,
}

impl MaterialSpec {
    /// A plain catalogue material by name, nothing linked or overridden.
    #[must_use]
    pub fn catalogue(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            custom_materials: Vec::new(),
            linked_design: None,
            overrides: MaterialOverridesSpec::default(),
        }
    }
}

/// The orbit camera (the field of view is fixed at [`DEFAULT_FOV_DEG`], like the
/// desktop's).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraSpec {
    /// Orbit yaw in radians.
    pub yaw: f32,
    /// Orbit pitch in radians.
    pub pitch: f32,
    /// Distance from the origin.
    pub distance: f32,
}

/// Studio lighting: preset, exposure, light direction and backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LightingSpec {
    /// `LightingPreset::index` (0-14; see `LightingPreset::ALL`).
    pub preset_index: i32,
    /// Exposure multiplier.
    pub exposure: f32,
    /// Light yaw in radians (used by the presets that have a movable light).
    pub light_yaw: f32,
    /// Light pitch in radians.
    pub light_pitch: f32,
    /// `render_setup::Backdrop::index` (0 as lit, 1 grey, 2 white).
    pub backdrop_index: i32,
    /// Head-shadow radius in degrees of the lit presets (`0.0` off, default `16.0`,
    /// `EnvironmentSource::Studio::head_shadow_deg`); appended in protocol v11.
    pub head_shadow_deg: f32,
}

impl LightingSpec {
    /// Builds a spec from typed values.
    #[must_use]
    pub const fn new(
        preset: LightingPreset,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
        backdrop: Backdrop,
        head_shadow_deg: f32,
    ) -> Self {
        Self {
            preset_index: preset.index(),
            exposure,
            light_yaw,
            light_pitch,
            backdrop_index: backdrop.index(),
            head_shadow_deg,
        }
    }

    /// The preset (out-of-range indices fall back like `LightingPreset::from_index`).
    #[must_use]
    pub const fn preset(&self) -> LightingPreset {
        LightingPreset::from_index(self.preset_index)
    }

    /// The backdrop (out-of-range indices fall back like `Backdrop::from_index`).
    #[must_use]
    pub const fn backdrop(&self) -> Backdrop {
        Backdrop::from_index(self.backdrop_index)
    }

    /// The analytic studio environment the desktop builds for these settings.
    #[must_use]
    pub const fn studio_environment(&self) -> EnvironmentSource<'static> {
        self.preset()
            .studio(self.exposure, self.light_yaw, self.light_pitch)
            .with_backdrop(self.backdrop().level())
            .with_head_shadow(self.head_shadow_deg)
    }
}

/// Everything a render Worker needs to trace a frame, as plain data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneSpec {
    /// The stone's facet planes.
    pub planes: Vec<PlaneData>,
    /// Facet finishes.
    pub finishes: FinishSpec,
    /// The material.
    pub material: MaterialSpec,
    /// The camera.
    pub camera: CameraSpec,
    /// Studio lighting (ignored for the environment when `hdr_id` is set, as on the
    /// desktop).
    pub lighting: LightingSpec,
    /// Maximum ray bounces.
    pub max_bounces: u32,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// The HDR map lighting this scene (sent earlier as `ToWorker::HdrMap`), or `None`
    /// for the studio rig.
    pub hdr_id: Option<u64>,
}

impl SceneSpec {
    /// `width * height`.
    #[must_use]
    pub const fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Why a [`SceneSpec`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneError {
    /// The spec has no planes, so there is no stone.
    NoPlanes,
    /// A zero or oversized frame (see [`MAX_FRAME_PIXELS`]).
    BadFrameSize {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// The material name resolves to nothing in the catalogue or the custom list.
    UnknownMaterial {
        /// The name that did not resolve.
        name: String,
    },
    /// The spec names an HDR map the Worker does not hold.
    MissingHdr {
        /// The missing map's id.
        id: u64,
    },
    /// A per-facet finish list longer than the plane list.
    TooManyFinishes {
        /// Finish entries.
        finishes: usize,
        /// Planes.
        planes: usize,
    },
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPlanes => write!(f, "the scene has no facet planes to render"),
            Self::BadFrameSize { width, height } => write!(
                f,
                "a {width}x{height} frame is outside the supported size (1 to \
                 {MAX_FRAME_PIXELS} pixels)"
            ),
            Self::UnknownMaterial { name } => write!(
                f,
                "the material '{name}' is not a built-in preset or a loaded custom material"
            ),
            Self::MissingHdr { id } => write!(
                f,
                "the scene uses HDR map {id}, which this worker has not loaded"
            ),
            Self::TooManyFinishes { finishes, planes } => write!(
                f,
                "{finishes} facet finishes were given for {planes} planes"
            ),
        }
    }
}

impl std::error::Error for SceneError {}

/// Resolves the traced material for `spec` exactly as the desktop does -- see the module
/// doc comment, step 1.
///
/// # Errors
///
/// [`SceneError::UnknownMaterial`] when neither the linked override nor the name
/// resolves.
pub fn resolve_scene_material(
    spec: &MaterialSpec,
    planes: &[GpuFacetPlane],
) -> Result<GemMaterial, SceneError> {
    resolve_scene_material_with(spec, planes, false)
}

/// [`resolve_scene_material`] with an explicit `physics_color` flag.
///
/// The flag says whether the traced material's color is a physics recipe. It comes from
/// [`CustomMaterialSpec::is_physics_color`] of the traced custom material, never from the
/// built material's bands. A physics material takes the 7 mm default stone width when
/// `overrides.stone_width_mm` is 0.
///
/// [`resolve_scene_material`] itself passes `false`: a [`MaterialSpec`] carries built
/// `GemMaterial`s only, so a caller that holds [`CustomMaterialSpec`]s calls this.
///
/// # Errors
///
/// [`SceneError::UnknownMaterial`] when neither the linked override nor the name
/// resolves.
pub fn resolve_scene_material_with(
    spec: &MaterialSpec,
    planes: &[GpuFacetPlane],
    physics_color: bool,
) -> Result<GemMaterial, SceneError> {
    let materials = GemMaterial::all_materials();
    let custom = &spec.custom_materials;
    let material_override = spec.linked_design.as_ref().and_then(|linked| {
        traced_gem_material(
            &spec.name,
            &linked.to_selection(),
            &EditorMaterialLookup::new(custom),
        )
    });
    let base =
        resolve_material_with_override(&materials, custom, material_override.as_ref(), &spec.name)
            .ok_or_else(|| SceneError::UnknownMaterial {
                name: spec.name.clone(),
            })?;
    let overrides = spec.overrides.to_overrides();
    let eff_stone_width = indicatrix::render_setup::materials::effective_stone_width_mm(
        overrides.stone_width_mm,
        physics_color,
    );
    let model_width = if eff_stone_width > 0.0 {
        measure_model_width(planes)
    } else {
        None
    };
    Ok(apply_material_overrides_for_mode(
        base,
        &overrides,
        model_width,
        physics_color,
    ))
}

/// The per-plane finishes for `spec` -- see the module doc comment, step 2.
///
/// # Errors
///
/// [`SceneError::TooManyFinishes`] for a [`FinishSpec::PerFacet`] list longer than the
/// plane list.
pub fn resolve_finishes(
    spec: &FinishSpec,
    planes: &[GpuFacetPlane],
) -> Result<Vec<FacetFinish>, SceneError> {
    match spec {
        FinishSpec::AllPolished => Ok(Vec::new()),
        FinishSpec::FrostedGirdle => Ok(girdle_facet_finishes(planes)),
        FinishSpec::PerFacet(frosted) => {
            if frosted.len() > planes.len() {
                return Err(SceneError::TooManyFinishes {
                    finishes: frosted.len(),
                    planes: planes.len(),
                });
            }
            Ok(frosted
                .iter()
                .map(|&f| {
                    if f {
                        FacetFinish::Frosted
                    } else {
                        FacetFinish::Polished
                    }
                })
                .collect())
        }
    }
}

/// The environment an [`OwnedScene`] traces against: the analytic rig, or a shared
/// HDR map.
enum OwnedEnvironment {
    Studio(EnvironmentSource<'static>),
    Hdr(Arc<EnvironmentMap>),
}

/// A built scene that owns its planes, finishes, material, camera and environment, and
/// lends out a `FrameScene` for the tracer.
///
/// Also owns the plane `SoA` arena the tracer's intersection scan needs
/// ([`Self::plane_soa`]), built once per scene rather than once per chunk.
pub struct OwnedScene {
    planes: Vec<GpuFacetPlane>,
    finishes: Vec<FacetFinish>,
    material: GemMaterial,
    camera: Camera,
    environment: OwnedEnvironment,
    max_bounces: u32,
    width: u32,
    height: u32,
    plane_soa: PlanesSoA32,
    /// The material's emitters; empty (the default) traces exactly as before.
    fluorescence: Fluorescence,
}

impl OwnedScene {
    /// Builds the scene `spec` describes, exactly as the desktop builds it for the same
    /// settings (see the module doc comment).
    ///
    /// `hdr` is the map for `spec.hdr_id` when that is set; it is shared, never copied
    /// (a decoded 8K map is hundreds of MiB). A map passed for a spec without `hdr_id`
    /// is ignored.
    ///
    /// # Errors
    ///
    /// See [`SceneError`].
    pub fn build(spec: &SceneSpec, hdr: Option<Arc<EnvironmentMap>>) -> Result<Self, SceneError> {
        if spec.planes.is_empty() {
            return Err(SceneError::NoPlanes);
        }
        if spec.width == 0 || spec.height == 0 || spec.pixel_count() > MAX_FRAME_PIXELS {
            return Err(SceneError::BadFrameSize {
                width: spec.width,
                height: spec.height,
            });
        }
        let planes: Vec<GpuFacetPlane> = spec.planes.iter().copied().map(Into::into).collect();
        let material = resolve_scene_material(&spec.material, &planes)?;
        let finishes = resolve_finishes(&spec.finishes, &planes)?;
        let camera = Camera::new(
            spec.camera.yaw,
            spec.camera.pitch,
            spec.camera.distance,
            DEFAULT_FOV_DEG,
        );
        let environment = match spec.hdr_id {
            Some(id) => OwnedEnvironment::Hdr(hdr.ok_or(SceneError::MissingHdr { id })?),
            None => OwnedEnvironment::Studio(spec.lighting.studio_environment()),
        };
        let plane_soa = build_plane_soa(&planes);
        Ok(Self {
            planes,
            finishes,
            material,
            camera,
            environment,
            max_bounces: spec.max_bounces,
            width: spec.width,
            height: spec.height,
            plane_soa,
            fluorescence: Fluorescence::new(Vec::new()),
        })
    }

    /// [`Self::build`] for a caller that holds the session's [`CustomMaterialSpec`]s (the
    /// materials with their physics recipes): the traced material's physics flag decides the
    /// default stone width (`resolve_scene_material_with`) and its recipe fills the scene's
    /// fluorescence ([`CustomMaterialSpec::fluorescence`]), the way the desktop's render
    /// context carries `active_fluorescence` beside `physics_color()`. A traced material that
    /// is not among `customs` (a catalogue name) builds exactly like [`Self::build`].
    ///
    /// # Errors
    ///
    /// See [`SceneError`].
    pub fn build_with_customs(
        spec: &SceneSpec,
        hdr: Option<Arc<EnvironmentMap>>,
        customs: &[CustomMaterialSpec],
    ) -> Result<Self, SceneError> {
        let traced = customs
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&spec.material.name));
        let mut scene = Self::build(spec, hdr)?;
        if let Some(custom) = traced.filter(|c| c.is_physics_color()) {
            scene.material = resolve_scene_material_with(&spec.material, &scene.planes, true)?;
            scene.fluorescence = custom.fluorescence();
        }
        Ok(scene)
    }

    /// Replaces the scene's fluorescence (empty for none).
    #[must_use]
    pub fn with_fluorescence(mut self, fluorescence: Fluorescence) -> Self {
        self.fluorescence = fluorescence;
        self
    }

    /// The material's fluorescent emitters; empty for a non-fluorescent material. A fluorescent
    /// scene is traced on the CPU alone (`scene_routes_to_gpu`), which is all the browser has.
    #[must_use]
    pub const fn fluorescence(&self) -> &Fluorescence {
        &self.fluorescence
    }

    /// The borrowed `FrameScene` the tracer takes.
    #[must_use]
    pub fn frame_scene(&self) -> FrameScene<'_> {
        FrameScene {
            camera: &self.camera,
            width: self.width,
            height: self.height,
            planes: &self.planes,
            facet_finishes: &self.finishes,
            material: &self.material,
            max_bounces: self.max_bounces,
            environment: match &self.environment {
                OwnedEnvironment::Studio(source) => *source,
                OwnedEnvironment::Hdr(map) => EnvironmentSource::HdrMap(map),
            },
        }
    }

    /// The plane `SoA` arena for [`crate::render::handle_trace_chunk`].
    #[must_use]
    pub const fn plane_soa(&self) -> &PlanesSoA32 {
        &self.plane_soa
    }

    /// The resolved material.
    #[must_use]
    pub const fn material(&self) -> &GemMaterial {
        &self.material
    }

    /// The camera.
    #[must_use]
    pub const fn camera(&self) -> &Camera {
        &self.camera
    }

    /// The planes.
    #[must_use]
    pub fn planes(&self) -> &[GpuFacetPlane] {
        &self.planes
    }

    /// The resolved per-plane finishes (empty = all polished).
    #[must_use]
    pub fn finishes(&self) -> &[FacetFinish] {
        &self.finishes
    }

    /// Frame width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Frame height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Maximum bounces.
    #[must_use]
    pub const fn max_bounces(&self) -> u32 {
        self.max_bounces
    }
}

#[cfg(test)]
mod physics_flag_tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
    };
    use indicatrix_cut_core::{material::ColorMode, native::color_recipe_dto};

    fn spec_with(mode: Option<&ColorMode>, rgb: [f32; 3]) -> CustomMaterialSpec {
        CustomMaterialSpec {
            name: "Ruby X".to_string(),
            mean_ri: 1.77,
            dispersion_delta: 0.018,
            birefringence_delta: -0.008,
            absorption_rgb: Some(rgb.map(f64::from)),
            color_recipe: mode.map(color_recipe_dto),
            absorption_bands: Vec::new(),
        }
    }

    fn pure_host_mode() -> ColorMode {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = ColorRecipe::new("corundum", cat.data_version);
        let (tensor, _) = resolve(&recipe, cat).expect("pure host resolves");
        recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
        ColorMode::physics(recipe, [0.0; 3])
    }

    /// The flag follows the recipe the spec carries, not the shape of the built bands: a
    /// pure-host recipe (no bands at all) is still physics.
    #[test]
    fn the_flag_follows_the_recipe_presence() {
        let mode = pure_host_mode();
        let fallback = mode.fallback_rgb();
        assert!(spec_with(Some(&mode), fallback).is_physics_color());
        assert!(!spec_with(None, [0.0; 3]).is_physics_color());
        // A fantasy-active material parks its recipe but renders as fantasy.
        let mut parked = mode.clone();
        parked.switch_to_fantasy();
        assert!(!spec_with(Some(&parked), parked.fallback_rgb()).is_physics_color());
        // An older build edited the top-level color: not physics until the user chooses.
        assert!(!spec_with(Some(&mode), [0.1, 0.2, 2.5]).is_physics_color());
    }

    /// A zero-band physics recipe still gets the 7 mm default width; without the flag the
    /// material keeps scale 1.0.
    #[test]
    fn a_pure_host_physics_material_takes_the_default_stone_width() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mode = pure_host_mode();
        let spec = spec_with(Some(&mode), mode.fallback_rgb());
        assert!(spec.is_physics_color());
        let material_spec = MaterialSpec {
            custom_materials: vec![spec.to_gem_material()],
            ..MaterialSpec::catalogue("Ruby X")
        };
        let plain = resolve_scene_material_with(&material_spec, &planes, false).expect("resolves");
        assert!((plain.absorption_path_scale - 1.0).abs() < f32::EPSILON);
        let physics = resolve_scene_material_with(&material_spec, &planes, true).expect("resolves");
        assert!(
            (physics.absorption_path_scale - 1.0).abs() > 1e-3,
            "7 mm / model width must rescale the path, got {}",
            physics.absorption_path_scale
        );
    }

    /// A physics ruby recipe fills the scene's fluorescence (`build_with_customs`), a catalogue
    /// material or a fantasy color leaves it empty, and the scene then traces through
    /// `handle_trace_chunk` with the emitters beside it.
    #[test]
    fn a_physics_recipe_fills_the_scenes_fluorescence() {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = ColorRecipe::new("corundum", cat.data_version);
        recipe.set_amount("Cr", 0.5);
        let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
        recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
        let mode = ColorMode::physics(recipe, [0.0; 3]);
        let custom = spec_with(Some(&mode), mode.fallback_rgb());
        assert_eq!(custom.fluorescence().emitters().len(), 1);
        assert!(spec_with(None, [0.0; 3]).fluorescence().is_empty());

        let scene_spec = |material: MaterialSpec| SceneSpec {
            planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
            finishes: FinishSpec::AllPolished,
            material,
            camera: CameraSpec {
                yaw: 0.6,
                pitch: 0.35,
                distance: 4.2,
            },
            lighting: LightingSpec::new(
                LightingPreset::UvLamp365,
                1.0,
                0.4,
                0.35,
                Backdrop::Grey,
                16.0,
            ),
            max_bounces: 4,
            width: 4,
            height: 4,
            hdr_id: None,
        };
        let ruby = scene_spec(MaterialSpec {
            custom_materials: vec![custom.to_gem_material()],
            ..MaterialSpec::catalogue("Ruby X")
        });
        let customs = [custom];
        let glowing = OwnedScene::build_with_customs(&ruby, None, &customs).expect("builds");
        assert!(!glowing.fluorescence().is_empty());
        let plain = OwnedScene::build(&ruby, None).expect("builds");
        assert!(plain.fluorescence().is_empty());
        let diamond = OwnedScene::build_with_customs(
            &scene_spec(MaterialSpec::catalogue("Diamond")),
            None,
            &customs,
        )
        .expect("builds");
        assert!(diamond.fluorescence().is_empty());
        let chunk = crate::render::handle_trace_chunk(&glowing, glowing.plane_soa(), 0, 1, 0, 1);
        assert_eq!(chunk.len(), 16);
    }
}

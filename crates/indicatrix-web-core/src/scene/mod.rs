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
//!    colour applied through `MaterialSelection::apply_overrides`). Then
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
        materials::GemMaterial,
        raytracer::{
            Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish, LightingPreset,
            build_plane_soa,
        },
    },
    render_setup::{
        Backdrop, MaterialOverrides, apply_material_overrides, measure_model_width,
        resolve_material_with_override,
    },
    renderer::{env_map::EnvironmentMap, frame_scene::FrameScene},
    simd::PlanesSoA32,
};
use indicatrix_cut_core::{MaterialSelection, native::gem_material_from_custom_snapshot};
use indicatrix_editor::material_lookup::{EditorMaterialLookup, traced_gem_material};
use indicatrix_formats::native::CustomMaterialSnapshot;
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
    /// The body colour (the snapshot's `absorption_rgb`); `None` is colourless.
    pub absorption_rgb: Option<[f64; 3]>,
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
        }
    }

    /// Builds the material through `native::gem_material_from_custom_snapshot`, the
    /// same conversion the desktop uses when it restores a file's custom material.
    #[must_use]
    pub fn to_gem_material(&self) -> GemMaterial {
        let snapshot = CustomMaterialSnapshot::new(
            self.mean_ri,
            self.dispersion_delta,
            self.birefringence_delta,
            None,
            "",
            "",
        )
        .with_absorption_rgb(self.absorption_rgb);
        gem_material_from_custom_snapshot(&self.name, &snapshot)
    }
}

/// The linked design's per-design material overrides (`MaterialSelection`'s RI
/// override and body colour).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct DesignMaterialOverrides {
    /// `MaterialSelection::refractive_index_override`: a flat dispersion at this `n_d`.
    pub refractive_index_override: Option<f64>,
    /// `MaterialSelection::body_colour_override`: an isotropic body colour.
    pub body_colour_override: Option<[f32; 3]>,
}

impl DesignMaterialOverrides {
    /// Copies the two render-relevant overrides from a design's selection.
    #[must_use]
    pub const fn from_selection(selection: &MaterialSelection) -> Self {
        Self {
            refractive_index_override: selection.refractive_index_override,
            body_colour_override: selection.body_colour_override,
        }
    }

    fn to_selection(self) -> MaterialSelection {
        MaterialSelection {
            refractive_index_override: self.refractive_index_override,
            body_colour_override: self.body_colour_override,
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
    /// `LightingPreset::index` (0-6).
    pub preset_index: i32,
    /// Exposure multiplier.
    pub exposure: f32,
    /// Light yaw in radians (used by the presets that have a movable light).
    pub light_yaw: f32,
    /// Light pitch in radians.
    pub light_pitch: f32,
    /// `render_setup::Backdrop::index` (0 as lit, 1 grey, 2 white).
    pub backdrop_index: i32,
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
    ) -> Self {
        Self {
            preset_index: preset.index(),
            exposure,
            light_yaw,
            light_pitch,
            backdrop_index: backdrop.index(),
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
    let materials = GemMaterial::all_materials();
    let custom = &spec.custom_materials;
    let material_override = spec.linked_design.and_then(|linked| {
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
    // The desktop's `StoneWidthCache` adapter measures only when the control is on.
    let model_width = if overrides.stone_width_mm > 0.0 {
        measure_model_width(planes)
    } else {
        None
    };
    Ok(apply_material_overrides(base, &overrides, model_width))
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
        })
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

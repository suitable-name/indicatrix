//! Synthetic ground truth for the zoning validation (plan 2026-10-09, section 10.1; lane V1).
//!
//! Everything here goes through the PUBLIC API of `indicatrix-cut-core` (feature `zoning`), so the
//! tests and the benchmark example exercise exactly what the application calls. This file is
//! shared: `tests/zoning_validation/main.rs` declares it as a module and
//! `examples/zoning_bench.rs` includes it with `#[path]`.
//!
//! # What is generated
//!
//! * **Six roughs** ([`RoughKind`]): a cube, an irregular scanned-like mesh (a deterministic,
//!   noise-displaced icosphere), the same frosted, the same frosted with polished windows towards
//!   two cameras, a cube with a holder in the rig, and the irregular mesh in immersion
//!   (`surround_n` equal to the stone index).
//! * **Five colour cases** ([`ColourCase`]): uniform pale, uniform saturated, bicolour, a
//!   "watermelon" prism core, a sector. The spectra are sums of Gaussian bands of PHYSICAL width
//!   (round 5, ZC-6): colour-bearing crystal-field absorptions have a FWHM of 2000 to 4000
//!   cm^-1, a wavelength sigma of 25 to 50 nm, which the 25 nm basis of the fit
//!   ([`FIT_BASIS_NM`]) can represent. They are still not exactly in the basis, so a recovery
//!   test also checks the model error, and every failure report prints the projection of the
//!   truth onto the basis ([`projection_errors`]). One case with 700 cm^-1 lines
//!   ([`ColourCase::NarrowLineBicolour`], the width of spin-forbidden lines, which carry little
//!   colour) is kept as an explicit model-class test, judged against its own projection.
//!   [`ColourCase::Banded`] (base, half space, prism and sector together) exists for the
//!   benchmark only.
//! * **Rig photos**, two independent ways:
//!   * from the forward model's own records ([`photos_forward`], with the noise of
//!     [`NoiseSpec`]) and a JPEG round trip ([`photos_jpeg`], decoded through the photometry
//!     lane);
//!   * from the main CPU tracer ([`render_main`]) for the scenes it can express (a convex cube
//!     in a uniform white furnace), which is the independent cross-check of section 10.1.
//!
//! # Conventions
//!
//! Mesh frame equals rig frame (identity alignment), millimetres. The main tracer works in model
//! units: one model unit is [`MM_PER_MODEL_UNIT`] millimetres, set through
//! `GemMaterial::with_absorption_path_scale`.

#![allow(
    dead_code,
    reason = "a shared fixture module: each test target and the example use a subset"
)]

use std::{
    collections::BTreeMap,
    f64::consts::{FRAC_PI_2, TAU},
    sync::atomic::AtomicBool,
};

use glam::{DVec2, DVec3, Vec3};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use indicatrix::{
    color::{
        body_color::{Illuminant, body_color, delta_e_2000},
        cie1931::CIE_1931_Y_INTEGRAL_5NM,
    },
    geometry::GpuFacetPlane,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        raytracer::{EnvironmentSource, Ray, cie_1931_cmf, hash_u32, trace_spectral_ray},
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    render_setup::MODEL_UNIT_FACE_UP_PATH,
    renderer::env_map::{EnvironmentMap, rgb_to_spectral_radiance},
};
use indicatrix_cut_core::rough_plan::{
    camera_spectral::{BacklightSpectrum, CameraResponse, GRID_FIRST_NM, GRID_LEN, GRID_STEP_NM},
    colour_fit::{
        ColourRig,
        forward::{
            ForwardInput, ForwardOptions, ForwardRecords, PanelGeom, RigLighting, StoneIndex,
            SurfaceClass, SurfaceMap, ViewTraceInput, evaluate, trace_rig,
        },
        solve::{ColourFit, FIT_BASIS_NM, FitConfig, ObservedView, SmoothBasisModel},
    },
    locate::{Projection, RigProfile, Rigid, Scene, ViewPose, box_mesh},
    photometry::{
        CalibrationFrames, JPEG_BLOCK, LinearImage, NoiseModel, PixelMask, ResampleOptions,
        TransmittanceOptions, ViewCalibration, WorkingGrid, decode_standard_bytes, flag,
        linear_to_srgb,
    },
    shape::RoughMesh,
};

// ---------------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------------

/// The stone's refractive index in the rig (no dispersion in the validation scenes).
pub const STONE_N: f64 = 1.6;
/// Millimetres per model unit of the main tracer's cube.
pub const MM_PER_MODEL_UNIT: f32 = 2.0;
/// The level of the white (empty backlight) frame in the photos, linear, 0 to 1.
pub const WHITE_LEVEL: f64 = 0.7;
/// Camera positions around a 10 mm cube at the origin (up is +Z; none looks along it). The same
/// four as the solver's own tests.
pub const POSITIONS: [(f64, f64, f64); 4] = [
    (0.0, -100.0, 0.0),
    (-70.0, -70.0, 30.0),
    (70.0, -70.0, -30.0),
    (0.0, -80.0, 60.0),
];
/// Per-view gain (exposure drift) of the synthetic photos.
pub const GAINS: [f64; 8] = [1.0, 0.98, 1.02, 1.0, 0.99, 1.01, 1.0, 0.98];
/// The stone widths the face-up colour is checked at, mm.
pub const SIZES_MM: [f64; 2] = [7.0, 12.0];

/// The two illuminants of the recovery criteria: D65 and A.
#[must_use]
pub const fn illuminants() -> [Illuminant; 2] {
    [Illuminant::D65, Illuminant::Planckian(2856.0)]
}

/// A short label of an illuminant for failure messages.
#[must_use]
pub const fn illuminant_label(illuminant: Illuminant) -> &'static str {
    match illuminant {
        Illuminant::D65 => "D65",
        _ => "A",
    }
}

// ---------------------------------------------------------------------------------------------
// Deterministic random numbers
// ---------------------------------------------------------------------------------------------

/// `SplitMix64` with Box-Muller normals: the seeded noise of the synthetic photos.
pub struct SplitMix(pub u64);

impl SplitMix {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform number in the open interval (0, 1).
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    }

    /// A standard normal number.
    pub fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }
}

// ---------------------------------------------------------------------------------------------
// Roughs
// ---------------------------------------------------------------------------------------------

/// The six rough scenarios of section 10.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoughKind {
    /// A 10 mm cube, polished.
    Cube,
    /// A scanned-like irregular mesh (noise-displaced icosphere), polished.
    Irregular,
    /// The irregular mesh with a frosted (GGX) skin.
    Frosted,
    /// The frosted mesh with polished windows towards the first two cameras.
    PolishedWindows,
    /// The cube with a holder (a post below it) in the rig.
    WithHolder,
    /// The irregular mesh immersed in a liquid of the stone's index.
    Immersion,
}

impl RoughKind {
    /// Every rough, in the order of the section.
    pub const ALL: [Self; 6] = [
        Self::Cube,
        Self::Irregular,
        Self::Frosted,
        Self::PolishedWindows,
        Self::WithHolder,
        Self::Immersion,
    ];

    /// A label for messages.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cube => "cube",
            Self::Irregular => "irregular",
            Self::Frosted => "frosted",
            Self::PolishedWindows => "polished windows",
            Self::WithHolder => "holder",
            Self::Immersion => "immersion",
        }
    }

    /// Whether the main CPU tracer can render the scene (a convex polyhedron in air).
    #[must_use]
    pub const fn main_tracer_can_render(self) -> bool {
        matches!(self, Self::Cube)
    }
}

/// The 20 triangles of an icosahedron over the 12 vertices of [`icosphere_unit`], wound outward.
const ICOSAHEDRON_TRIANGLES: [[u32; 3]; 20] = [
    [0, 11, 5],
    [0, 5, 1],
    [0, 1, 7],
    [0, 7, 10],
    [0, 10, 11],
    [1, 5, 9],
    [5, 11, 4],
    [11, 10, 2],
    [10, 7, 6],
    [7, 1, 8],
    [3, 9, 4],
    [3, 4, 2],
    [3, 2, 6],
    [3, 6, 8],
    [3, 8, 9],
    [4, 9, 5],
    [2, 4, 11],
    [6, 2, 10],
    [8, 6, 7],
    [9, 8, 1],
];

/// A fixture size or index as `u32`; the fixture meshes and pictures are far below `u32::MAX`.
fn u32_of(n: usize) -> u32 {
    u32::try_from(n).expect("a fixture size fits u32")
}

/// An f64 value narrowed to the f32 the rig and tracer APIs take.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the precision of the APIs under test; the fixture values are in the normal float range"
)]
const fn f32_of(v: f64) -> f32 {
    v as f32
}

/// A rounded, in-range channel value as a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass a value rounded within 0..=255; the saturating cast is the intent"
)]
const fn byte_of(v: f32) -> u8 {
    v as u8
}

fn edge_midpoint(
    vertices: &mut Vec<DVec3>,
    cache: &mut BTreeMap<(u32, u32), u32>,
    a: u32,
    b: u32,
) -> u32 {
    let key = (a.min(b), a.max(b));
    if let Some(&index) = cache.get(&key) {
        return index;
    }
    let middle = ((vertices[a as usize] + vertices[b as usize]) * 0.5).normalize();
    let index = u32_of(vertices.len());
    vertices.push(middle);
    cache.insert(key, index);
    index
}

/// A unit icosphere with `levels` midpoint subdivisions (20, 80, 320, ... triangles).
#[must_use]
pub fn icosphere_unit(levels: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let phi = f64::midpoint(1.0, 5.0_f64.sqrt());
    let mut vertices: Vec<DVec3> = [
        DVec3::new(-1.0, phi, 0.0),
        DVec3::new(1.0, phi, 0.0),
        DVec3::new(-1.0, -phi, 0.0),
        DVec3::new(1.0, -phi, 0.0),
        DVec3::new(0.0, -1.0, phi),
        DVec3::new(0.0, 1.0, phi),
        DVec3::new(0.0, -1.0, -phi),
        DVec3::new(0.0, 1.0, -phi),
        DVec3::new(phi, 0.0, -1.0),
        DVec3::new(phi, 0.0, 1.0),
        DVec3::new(-phi, 0.0, -1.0),
        DVec3::new(-phi, 0.0, 1.0),
    ]
    .iter()
    .map(|v| v.normalize())
    .collect();
    let mut triangles = ICOSAHEDRON_TRIANGLES.to_vec();
    for _ in 0..levels {
        let mut cache = BTreeMap::new();
        let mut next = Vec::with_capacity(triangles.len() * 4);
        for &[a, b, c] in &triangles {
            let ab = edge_midpoint(&mut vertices, &mut cache, a, b);
            let bc = edge_midpoint(&mut vertices, &mut cache, b, c);
            let ca = edge_midpoint(&mut vertices, &mut cache, c, a);
            next.push([a, ab, ca]);
            next.push([b, bc, ab]);
            next.push([c, ca, bc]);
            next.push([ab, bc, ca]);
        }
        triangles = next;
    }
    (vertices, triangles)
}

/// The scanned-like rough: an icosphere of mean radius `radius_mm` whose radius varies by a
/// smooth low-frequency field (about 12 percent) plus 1.5 percent per-vertex noise. Radial
/// displacement keeps it star-shaped, so it is a valid closed mesh; the smallest radius stays
/// above 0.86 of the mean. Deterministic.
#[must_use]
pub fn scanned_like_mesh(radius_mm: f64, seed: u64) -> RoughMesh {
    let (unit, triangles) = icosphere_unit(2);
    let mut rng = SplitMix::new(seed);
    let points: Vec<DVec3> = unit
        .iter()
        .map(|u| {
            let smooth = 0.05f64.mul_add(
                2.3f64.mul_add(u.z, 0.6).sin(),
                0.07 * 1.9f64.mul_add(u.x, 1.3).sin() * (1.5 * u.y).cos(),
            );
            let noise = rng.uniform() - 0.5;
            *u * radius_mm * 0.03f64.mul_add(noise, 1.0 + smooth)
        })
        .collect();
    RoughMesh::new(&points, &triangles).expect("a closed star-shaped mesh")
}

/// The frosted surface with polished windows on the triangles that face the first two cameras.
fn polished_windows(mesh: &RoughMesh) -> SurfaceMap {
    let mut map = SurfaceMap::frosted(0.25);
    let directions: Vec<DVec3> = POSITIONS[..2]
        .iter()
        .map(|&(x, y, z)| DVec3::new(x, y, z).normalize())
        .collect();
    let vertices = mesh.vertices();
    for (index, triangle) in mesh.triangles().iter().enumerate() {
        let [a, b, c] = triangle.map(|i| vertices[i as usize]);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if directions.iter().any(|d| normal.dot(*d) > 0.8) {
            map = map.with_override(u32_of(index), SurfaceClass::Polished);
        }
    }
    map
}

/// A rough with its skin, holder and surrounding medium.
pub struct Rough {
    pub kind: RoughKind,
    pub mesh: RoughMesh,
    pub surfaces: SurfaceMap,
    /// The holder in the rig frame.
    pub holder: Option<RoughMesh>,
    /// The refractive index around the stone.
    pub surround_n: f64,
}

impl Rough {
    /// The rough of `kind`; `scale` multiplies all lengths (1 gives the 10 mm cube and the
    /// 14 mm irregular stone).
    #[must_use]
    pub fn build(kind: RoughKind, scale: f64) -> Self {
        let cube = || box_mesh(DVec3::splat(5.0 * scale)).expect("a cube");
        let scan = || scanned_like_mesh(7.0 * scale, 0x05CA_11ED);
        let polished = SurfaceMap::polished();
        match kind {
            RoughKind::Cube => Self {
                kind,
                mesh: cube(),
                surfaces: polished,
                holder: None,
                surround_n: 1.0,
            },
            RoughKind::Irregular => Self {
                kind,
                mesh: scan(),
                surfaces: polished,
                holder: None,
                surround_n: 1.0,
            },
            RoughKind::Frosted => Self {
                kind,
                mesh: scan(),
                surfaces: SurfaceMap::frosted(0.2),
                holder: None,
                surround_n: 1.0,
            },
            RoughKind::PolishedWindows => {
                let mesh = scan();
                let surfaces = polished_windows(&mesh);
                Self {
                    kind,
                    mesh,
                    surfaces,
                    holder: None,
                    surround_n: 1.0,
                }
            }
            RoughKind::WithHolder => {
                let holder = box_mesh(DVec3::new(1.5, 1.5, 3.0) * scale)
                    .expect("a post")
                    .translated(DVec3::new(0.0, 0.0, -9.0 * scale))
                    .expect("a translated post");
                Self {
                    kind,
                    mesh: cube(),
                    surfaces: polished,
                    holder: Some(holder),
                    surround_n: 1.0,
                }
            }
            RoughKind::Immersion => Self {
                kind,
                mesh: scan(),
                surfaces: polished,
                holder: None,
                surround_n: STONE_N,
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Colour cases and their ground truth
// ---------------------------------------------------------------------------------------------

/// The five colour cases of section 10.1 (plus [`Self::Banded`] for the benchmark).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourCase {
    UniformPale,
    UniformSaturated,
    /// Base plus the half space `x >= offset`.
    Bicolour,
    /// A trigonal prism core (axis Z, apothem 2 mm) in a differently coloured rim.
    WatermelonPrism,
    /// A 90 degree wedge about Z.
    Sector,
    /// The bicolour base with 700 cm^-1 lines (sigma 9 to 12 nm), narrower than anything the
    /// fit basis can represent: a model-class case, not part of [`Self::ALL`]. Its criterion is
    /// the projection of the truth onto the basis plus the RAW limit (see `recovery.rs`).
    NarrowLineBicolour,
    /// Base, half space, prism core and sector: three shaped zones (the benchmark).
    Banded,
}

impl ColourCase {
    /// The five cases of the section, in order.
    pub const ALL: [Self; 5] = [
        Self::UniformPale,
        Self::UniformSaturated,
        Self::Bicolour,
        Self::WatermelonPrism,
        Self::Sector,
    ];

    /// A label for messages.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UniformPale => "uniform pale",
            Self::UniformSaturated => "uniform saturated",
            Self::Bicolour => "bicolour",
            Self::WatermelonPrism => "watermelon prism",
            Self::Sector => "sector",
            Self::NarrowLineBicolour => "bicolour with 700 cm^-1 lines",
            Self::Banded => "banded (4 zones)",
        }
    }
}

/// The FWHM, in cm^-1, of the colour-bearing bands of the truths: 2200 cm^-1 is a wavelength
/// sigma of 27 nm at 540 nm and 36 nm at 625 nm, inside the 25 to 50 nm of the crystal-field
/// absorptions of Cr3+, V3+, Fe2+/Ti4+ and Mn3+ (FWHM 2000 to 4000 cm^-1).
pub const PHYSICAL_FWHM_CM: f32 = 2200.0;
/// The FWHM, in cm^-1, of the lines of [`ColourCase::NarrowLineBicolour`] (sigma 9 to 12 nm).
pub const NARROW_LINE_FWHM_CM: f32 = 700.0;

/// The three-band red/orange/green absorption of the bicolour base, scaled by `scale`, with
/// bands of `fwhm_cm` (cm^-1): peaks 0.04, 0.03 and 0.024 per mm at 580, 625 and 540 nm.
#[must_use]
pub fn three_band_zone(scale: f32, fwhm_cm: f32) -> ZoneAbsorption {
    band_zone(&[
        (580.0, fwhm_cm, 0.04 * scale),
        (625.0, fwhm_cm, 0.03 * scale),
        (540.0, fwhm_cm, 0.024 * scale),
    ])
}

/// A zone absorption from `(centre nm, FWHM cm^-1, peak per mm)` Gaussian energy bands
/// ([`AbsorptionBand::energy`]).
#[must_use]
pub fn band_zone(bands: &[(f32, f32, f32)]) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(
        bands
            .iter()
            .map(|&(centre, width, peak)| AbsorptionBand::energy(centre, width, peak))
            .collect(),
    ))
}

/// A wavelength-independent absorption of `per_mm` (a very wide band: flat to 0.1 percent over
/// 380 to 780 nm); `0` gives a clear zone.
#[must_use]
pub fn neutral_zone(per_mm: f32) -> ZoneAbsorption {
    if per_mm <= 0.0 {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()))
    } else {
        band_zone(&[(550.0, 5000.0, per_mm)])
    }
}

/// The trigonal prism core of the watermelon case: axis Z through the origin, apothem 2 mm.
#[must_use]
pub const fn watermelon_prism() -> ZoneShape {
    ZoneShape::CoaxialPrism {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        n_sides: 3,
        r_in: 0.0,
        r_out: WATERMELON_APOTHEM_MM,
        phase: 0.0,
    }
}

/// The apothem of [`watermelon_prism`], mm.
pub const WATERMELON_APOTHEM_MM: f64 = 2.0;

/// The 90 degree wedge about Z of the sector case.
#[must_use]
pub const fn sector_wedge() -> ZoneShape {
    ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        angle_from: 0.0,
        angle_to: FRAC_PI_2,
    }
}

/// The true absorption and zone geometry of a scene (mesh frame, mm, identity frame).
#[derive(Debug, Clone)]
pub struct Truth {
    pub case: Option<ColourCase>,
    /// The zone geometry with the spectra; `None` for a uniform stone.
    pub geometry: Option<ZonedAbsorption>,
    /// The spectrum of a uniform stone (zone 0).
    pub uniform: Option<ZoneAbsorption>,
}

impl Truth {
    /// The ground truth of `case`; `offset_mm` is the position of the bicolour boundary.
    #[must_use]
    pub fn new(case: ColourCase, offset_mm: f64) -> Self {
        let zoned = |base: ZoneAbsorption, zones: Vec<Zone>| {
            let mut z = ZonedAbsorption::new(base);
            z.zones = zones;
            Some(z)
        };
        let half_space = |absorption: ZoneAbsorption| Zone {
            shape: ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: offset_mm,
            },
            absorption,
        };
        let (geometry, uniform) = match case {
            ColourCase::UniformPale => (
                None,
                Some(band_zone(&[(545.0, 2600.0, 0.02), (390.0, 3500.0, 0.016)])),
            ),
            ColourCase::UniformSaturated => (
                None,
                Some(band_zone(&[(545.0, 2600.0, 0.2), (390.0, 3500.0, 0.16)])),
            ),
            ColourCase::Bicolour | ColourCase::NarrowLineBicolour => {
                let fwhm = if case == ColourCase::NarrowLineBicolour {
                    NARROW_LINE_FWHM_CM
                } else {
                    PHYSICAL_FWHM_CM
                };
                (
                    zoned(
                        three_band_zone(1.0, fwhm),
                        vec![half_space(band_zone(&[
                            (545.0, 2600.0, 0.2),
                            (390.0, 3500.0, 0.16),
                        ]))],
                    ),
                    None,
                )
            }
            ColourCase::WatermelonPrism => (
                zoned(
                    band_zone(&[(545.0, 2600.0, 0.1), (390.0, 3500.0, 0.08)]),
                    vec![Zone {
                        shape: watermelon_prism(),
                        absorption: three_band_zone(2.0, PHYSICAL_FWHM_CM),
                    }],
                ),
                None,
            ),
            ColourCase::Sector => (
                zoned(
                    band_zone(&[(545.0, 2600.0, 0.04), (390.0, 3500.0, 0.032)]),
                    vec![Zone {
                        shape: sector_wedge(),
                        absorption: three_band_zone(1.5, PHYSICAL_FWHM_CM),
                    }],
                ),
                None,
            ),
            ColourCase::Banded => (
                zoned(
                    band_zone(&[(545.0, 2600.0, 0.04), (390.0, 3500.0, 0.032)]),
                    vec![
                        half_space(band_zone(&[(545.0, 2600.0, 0.2), (390.0, 3500.0, 0.16)])),
                        Zone {
                            shape: watermelon_prism(),
                            absorption: three_band_zone(2.0, PHYSICAL_FWHM_CM),
                        },
                        Zone {
                            shape: sector_wedge(),
                            absorption: three_band_zone(1.0, PHYSICAL_FWHM_CM),
                        },
                    ],
                ),
                None,
            ),
        };
        Self {
            case: Some(case),
            geometry,
            uniform,
        }
    }

    /// A wavelength-independent stone: `base_per_mm` everywhere, or, with `zone_per_mm`, that
    /// value on the half space `x >= offset_mm`. For the cross-check against the main tracer.
    #[must_use]
    pub fn neutral(base_per_mm: f32, zone_per_mm: Option<f32>, offset_mm: f64) -> Self {
        zone_per_mm.map_or_else(
            || Self {
                case: None,
                geometry: None,
                uniform: Some(neutral_zone(base_per_mm)),
            },
            |zone| {
                let mut z = ZonedAbsorption::new(neutral_zone(base_per_mm));
                z.zones.push(Zone {
                    shape: ZoneShape::HalfSpace {
                        normal: DVec3::X,
                        offset: offset_mm,
                    },
                    absorption: neutral_zone(zone),
                });
                Self {
                    case: None,
                    geometry: Some(z),
                    uniform: None,
                }
            },
        )
    }

    /// The number of zones, the base included.
    #[must_use]
    pub fn zone_count(&self) -> usize {
        self.geometry.as_ref().map_or(1, |g| 1 + g.zones.len())
    }

    /// The absorption of `zone` (0 is the base).
    ///
    /// # Panics
    ///
    /// For a zone that does not exist.
    #[must_use]
    pub fn zone(&self, zone: usize) -> &ZoneAbsorption {
        self.geometry.as_ref().map_or_else(
            || self.uniform.as_ref().expect("a uniform spectrum"),
            |g| g.zone_absorption(zone).expect("a zone of the truth"),
        )
    }

    /// The absorption coefficient of `zone` at `lambda_nm`, per mm.
    #[must_use]
    pub fn alpha(&self, zone: usize, lambda_nm: f64) -> f64 {
        self.zone(zone).alpha(lambda_nm, None).max(0.0)
    }

    /// The CIELAB colour of a stone made of `zone` alone, `size_mm` wide, under `illuminant`:
    /// the solver's own convention (`MODEL_UNIT_FACE_UP_PATH` times the width, millimetres).
    #[must_use]
    pub fn face_up_lab(&self, zone: usize, size_mm: f64, illuminant: Illuminant) -> [f64; 3] {
        let path = f64::from(MODEL_UNIT_FACE_UP_PATH) * size_mm;
        body_color(|lambda| self.alpha(zone, lambda), path, illuminant).lab
    }
}

/// How far a fit's face-up colour is from the truth for one zone, size and illuminant.
#[derive(Debug, Clone, Copy)]
pub struct FaceUpError {
    pub zone: usize,
    pub size_mm: f64,
    pub illuminant: Illuminant,
    pub delta_e: f64,
}

/// CIEDE2000 between the fitted and the true face-up colour of every zone at 7 and 12 mm under
/// D65 and A.
///
/// # Panics
///
/// When the fit holds no prediction for a combination (the default `PredictionConfig` has all).
#[must_use]
pub fn face_up_errors(fit: &ColourFit, truth: &Truth) -> Vec<FaceUpError> {
    let mut out = Vec::new();
    for zone in 0..truth.zone_count() {
        for size_mm in SIZES_MM {
            for illuminant in illuminants() {
                let prediction = fit
                    .prediction(zone, size_mm, illuminant)
                    .expect("the default prediction config covers 7 and 12 mm under D65 and A");
                out.push(FaceUpError {
                    zone,
                    size_mm,
                    illuminant,
                    delta_e: delta_e_2000(
                        truth.face_up_lab(zone, size_mm, illuminant),
                        prediction.lab,
                    ),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The truth projected onto the fit basis (model class)
// ---------------------------------------------------------------------------------------------

/// The non-negative amplitudes (per mm) of [`FIT_BASIS_NM`] that best represent the absorption
/// of `zone` of the truth in the least-squares sense on the 5 nm grid from 400 to 700 nm: the
/// same grid and norm as the solver's own projection, so this IS the closest spectrum the fit
/// can reach. Cyclic coordinate descent on the 14 amplitudes (a fixed number of sweeps, so it is
/// deterministic).
#[must_use]
pub fn projected_amplitudes(truth: &Truth, zone: usize) -> Vec<f64> {
    let bands = FIT_BASIS_NM.len();
    let grid: Vec<f64> = (0..=60)
        .map(|i| 5.0_f64.mul_add(f64::from(i), 400.0))
        .collect();
    let target: Vec<f64> = grid.iter().map(|&l| truth.alpha(zone, l)).collect();
    let table: Vec<Vec<f64>> = (0..bands)
        .map(|k| {
            grid.iter()
                .map(|&l| SmoothBasisModel::basis(k, l))
                .collect()
        })
        .collect();
    let norms: Vec<f64> = table
        .iter()
        .map(|column| column.iter().map(|v| v * v).sum::<f64>())
        .collect();
    let mut amplitude = vec![0.0_f64; bands];
    let mut residual = target;
    for _ in 0..20_000 {
        let mut largest_step = 0.0_f64;
        for k in 0..bands {
            let gradient: f64 = table[k].iter().zip(&residual).map(|(b, r)| b * r).sum();
            let updated = (amplitude[k] + gradient / norms[k]).max(0.0);
            let step = updated - amplitude[k];
            if step != 0.0 {
                for (r, b) in residual.iter_mut().zip(&table[k]) {
                    *r -= step * b;
                }
                amplitude[k] = updated;
            }
            largest_step = largest_step.max(step.abs());
        }
        if largest_step < 1e-13 {
            break;
        }
    }
    amplitude
}

/// The absorption, per mm, of `amplitudes` on [`FIT_BASIS_NM`] at `lambda_nm`.
#[must_use]
pub fn projected_alpha(amplitudes: &[f64], lambda_nm: f64) -> f64 {
    amplitudes
        .iter()
        .enumerate()
        .map(|(k, a)| a * SmoothBasisModel::basis(k, lambda_nm))
        .sum()
}

/// The largest difference, per mm, between the truth of `zone` and its projection onto
/// [`FIT_BASIS_NM`] on the 5 nm grid from 400 to 700 nm.
#[must_use]
pub fn projection_max_alpha_error(truth: &Truth, zone: usize) -> f64 {
    let amplitudes = projected_amplitudes(truth, zone);
    (0..=60)
        .map(|i| {
            let lambda = 5.0_f64.mul_add(f64::from(i), 400.0);
            (truth.alpha(zone, lambda) - projected_alpha(&amplitudes, lambda)).abs()
        })
        .fold(0.0, f64::max)
}

/// CIEDE2000 between the true face-up colour of every zone at 7 and 12 mm under D65 and A and
/// the colour of the truth's best representation on [`FIT_BASIS_NM`] (round 5, ZC-6): the error a
/// PERFECT fit of the model class would still make. Same order as [`face_up_errors`]. A case
/// whose projection error is not far below the criterion tests the model class, not the solver.
#[must_use]
pub fn projection_errors(truth: &Truth) -> Vec<FaceUpError> {
    let mut out = Vec::new();
    for zone in 0..truth.zone_count() {
        let amplitudes = projected_amplitudes(truth, zone);
        for size_mm in SIZES_MM {
            let path = f64::from(MODEL_UNIT_FACE_UP_PATH) * size_mm;
            for illuminant in illuminants() {
                let projected = body_color(
                    |lambda| projected_alpha(&amplitudes, lambda),
                    path,
                    illuminant,
                )
                .lab;
                out.push(FaceUpError {
                    zone,
                    size_mm,
                    illuminant,
                    delta_e: delta_e_2000(truth.face_up_lab(zone, size_mm, illuminant), projected),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Noise and tiers
// ---------------------------------------------------------------------------------------------

/// The sensor noise model of the photometry lane: `variance = a + b * signal` in linear signal
/// units (0 to 1), with a white frame at [`WHITE_LEVEL`].
#[derive(Debug, Clone, Copy)]
pub struct NoiseSpec {
    pub a: f64,
    pub b: f64,
}

impl NoiseSpec {
    /// About 50 000 electrons of full well and a 2 electron read noise: `b = 1/50000`,
    /// `a = (2/50000)^2` rounded up (a RAW file of a good sensor).
    pub const RAW: Self = Self { a: 4e-6, b: 2e-5 };
    /// Practically noise-free (variance floor only).
    pub const NONE: Self = Self { a: 0.0, b: 0.0 };

    /// The variance of a signal.
    #[must_use]
    pub const fn signal_variance(&self, signal: f64) -> f64 {
        self.b.mul_add(signal.max(0.0), self.a)
    }

    /// The variance of a transmittance `t` measured against the white level.
    #[must_use]
    pub fn transmittance_variance(&self, t: f64) -> f64 {
        self.signal_variance(WHITE_LEVEL * t) / (WHITE_LEVEL * WHITE_LEVEL)
    }
}

/// The relative error the JPEG round trip may leave in a photo beyond the Gaussian noise: the
/// generator's flat-field tolerance (1.5 codes through the sRGB curve,
/// `a_flat_field_survives_the_jpeg_round_trip_within_quantisation`) at the brightest level it
/// checks, 1.5 %, the smallest tolerance over those levels (the darker ones allow more). It is
/// the JPEG tier's [`Tier::systematic`] figure.
pub const JPEG_SYSTEMATIC: f64 = 0.015;

/// The two accuracy tiers of section 10.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// RAW with a measured sensor response: noise only.
    Raw,
    /// The JPEG fallback: noise plus an 8-bit sRGB JPEG round trip.
    Jpeg,
}

impl Tier {
    /// The CIEDE2000 figure of plan section 10.1 (1.5 RAW, 3.0 JPEG). Since 2026-10-12
    /// `recovery.rs` asserts it as the floor of the metamer allowance and counts how often it is
    /// met on its own.
    #[must_use]
    pub const fn face_up_limit(self) -> f64 {
        match self {
            Self::Raw => 1.5,
            Self::Jpeg => 3.0,
        }
    }

    /// The relative systematic error of the tier's photos beyond the Gaussian noise, which the
    /// chi-square budget of `recovery.rs` allows the fit to chase: 0 for RAW (its photos are
    /// computed from the very records the fit evaluates) and [`JPEG_SYSTEMATIC`], 1.5 %, for
    /// JPEG (the generator's flat-field tolerance of one and a half 8-bit codes through the sRGB
    /// curve at the brightest level it checks).
    #[must_use]
    pub const fn systematic(self) -> f64 {
        match self {
            Self::Raw => 0.0,
            Self::Jpeg => JPEG_SYSTEMATIC,
        }
    }

    /// The limit of the leave-one-view-out median: 2.0 for both tiers, as plan section 10.1 lists
    /// it without a tier qualifier (round 5, ZC-16; the first version gave the JPEG tier 3.0).
    #[must_use]
    pub const fn lovo_limit(self) -> f64 {
        match self {
            Self::Raw | Self::Jpeg => 2.0,
        }
    }

    /// The working grid size: the JPEG variant needs a few 8 x 8 blocks.
    #[must_use]
    pub const fn grid_px(self) -> usize {
        match self {
            Self::Raw => 10,
            Self::Jpeg => 16,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The rig and the forward-model setup
// ---------------------------------------------------------------------------------------------

/// Where the light comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    /// A diffuse panel behind each view (the real rig).
    Backlight,
    /// A uniform all-round surround (the furnace; the main tracer's scene).
    Surround,
}

/// Which spectrum the backlight and the camera see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spectrum {
    /// A 5000 K CIE LED illuminant over 400 to 700 nm in 8 bins of 8 evaluation points (4.7
    /// nm). The table is verified against the CIE's 1 nm file (`indicatrix::color::led`).
    Led,
    /// The spectrum of the main tracer's white environment over 380 to 780 nm in 40 bins of 4
    /// evaluation points (2.5 nm), so that forward and main tracer weigh every wavelength alike.
    /// The midpoint rule's error for a 23 nm sigma band is nil at any spacing used here
    /// (aliasing `2 exp(-2 pi^2 sigma^2 / h^2)`, see `forward/mod.rs` "Spectral quadrature"),
    /// so a persistent cross-check bias is a transport question, not a quadrature one (ZC-2).
    EnvironmentWhite,
}

/// The knobs of a [`Setup`].
#[derive(Debug, Clone)]
pub struct SetupSpec {
    pub rough: RoughKind,
    pub colour: ColourCase,
    /// The position of the bicolour boundary, mm.
    pub offset_mm: f64,
    /// Number of views (the first four are [`POSITIONS`], more are a ring).
    pub views: usize,
    /// Explicit camera positions; replaces `views` when set.
    pub positions: Option<Vec<(f64, f64, f64)>>,
    /// Maximum working-grid size (pixels across).
    pub grid_px: usize,
    /// Camera-ray samples per pixel.
    pub samples: usize,
    pub threads: usize,
    /// Scale of the rough and the holder (1: 10 mm cube).
    pub scale: f64,
    pub px_per_mm: f64,
    pub image_px: u32,
    /// The stone region of the photo that is traced, `[x0, y0, x1, y1]`.
    pub window: [usize; 4],
    pub light: Light,
    pub spectrum: Spectrum,
    pub max_depth: usize,
    /// Trace the dispersion of corundum instead of the rig's constant index.
    pub dispersion: bool,
    pub seed: u64,
}

impl Default for SetupSpec {
    fn default() -> Self {
        Self {
            rough: RoughKind::Cube,
            colour: ColourCase::UniformSaturated,
            offset_mm: 0.3,
            views: 4,
            positions: None,
            grid_px: 10,
            samples: 64,
            threads: 2,
            scale: 1.0,
            px_per_mm: 10.0,
            image_px: 200,
            window: [60, 60, 140, 140],
            light: Light::Backlight,
            spectrum: Spectrum::Led,
            max_depth: 16,
            dispersion: false,
            seed: 0,
        }
    }
}

/// Camera positions for `n` views: the four of [`POSITIONS`] for `n <= 4`, otherwise a ring at
/// alternating elevations (100 mm from the origin, never along Z).
#[must_use]
pub fn view_positions(n: usize) -> Vec<(f64, f64, f64)> {
    if n <= POSITIONS.len() {
        return POSITIONS[..n].to_vec();
    }
    (0..n)
        .map(|i| {
            let azimuth = TAU * i as f64 / n as f64;
            let elevation: f64 = if i % 2 == 0 { 0.45 } else { -0.35 };
            (
                100.0 * elevation.cos() * azimuth.cos(),
                100.0 * elevation.cos() * azimuth.sin(),
                100.0 * elevation.sin(),
            )
        })
        .collect()
}

/// An orthographic camera at `position` looking at the origin, Z up.
#[must_use]
pub fn ortho_pose(position: DVec3, px_per_mm: f64, image_px: u32) -> ViewPose {
    ViewPose::look_at(
        "validation",
        position,
        DVec3::ZERO,
        DVec3::Z,
        Projection::Orthographic { px_per_mm },
        [image_px, image_px],
    )
}

/// The rg grid of the spectrum of the main tracer's white environment (radiance [1, 1, 1]).
#[must_use]
pub fn environment_white_grid() -> [f64; GRID_LEN] {
    let mut grid = [0.0; GRID_LEN];
    for (i, slot) in grid.iter_mut().enumerate() {
        let lambda = GRID_STEP_NM.mul_add(i as f64, GRID_FIRST_NM);
        *slot = f64::from(rgb_to_spectral_radiance([1.0, 1.0, 1.0], f32_of(lambda))).max(0.0);
    }
    grid
}

/// Everything [`trace_rig`] needs, owned.
pub struct Setup {
    pub spec: SetupSpec,
    pub rough: Rough,
    pub truth: Truth,
    pub rig: ColourRig,
    pub index: StoneIndex,
    pub camera: CameraResponse,
    pub backlight: BacklightSpectrum,
    pub options: ForwardOptions,
    pub views: Vec<ViewTraceInput<'static>>,
}

impl Setup {
    /// The setup of `spec` with the truth of `spec.colour`.
    #[must_use]
    pub fn new(spec: &SetupSpec) -> Self {
        Self::with_truth(spec, Truth::new(spec.colour, spec.offset_mm))
    }

    /// The setup of `spec` with this truth.
    ///
    /// # Panics
    ///
    /// When the spec cannot be built (a degenerate window or a missing spectrum).
    #[must_use]
    pub fn with_truth(spec: &SetupSpec, truth: Truth) -> Self {
        let rough = Rough::build(spec.rough, spec.scale);
        let positions = spec
            .positions
            .clone()
            .unwrap_or_else(|| view_positions(spec.views));
        let poses: Vec<ViewPose> = positions
            .iter()
            .map(|&(x, y, z)| ortho_pose(DVec3::new(x, y, z), spec.px_per_mm, spec.image_px))
            .collect();
        let lighting = match spec.light {
            Light::Backlight => {
                let panels: Vec<PanelGeom> = poses
                    .iter()
                    .map(|pose| PanelGeom::facing_camera(pose, 200.0, [400.0, 400.0]))
                    .collect();
                RigLighting::backlight(panels, (0..poses.len()).map(|_| None).collect())
            }
            Light::Surround => RigLighting::uniform_surround(),
        };
        let lighting = match &rough.holder {
            Some(holder) => lighting.with_holder(holder.clone()),
            None => lighting,
        };
        let view_count = poses.len();
        let mut profile = RigProfile::new("validation rig", poses, STONE_N);
        profile.surround_n = rough.surround_n;

        let (backlight, range, bins, sub) = match spec.spectrum {
            Spectrum::Led => (
                BacklightSpectrum::from_cct_k(5000.0).expect("an LED spectrum"),
                [400.0, 700.0],
                8,
                8,
            ),
            Spectrum::EnvironmentWhite => (
                BacklightSpectrum::from_grid(&environment_white_grid())
                    .expect("the white environment spectrum"),
                [380.0, 780.0],
                40,
                4,
            ),
        };
        let index = if spec.dispersion {
            StoneIndex::from_material(&GemMaterial::ruby())
        } else {
            StoneIndex::Rig
        };
        let grid = WorkingGrid::fit(spec.window, spec.grid_px).expect("a valid window");
        Self {
            spec: spec.clone(),
            rough,
            truth,
            rig: ColourRig::new(profile, lighting),
            index,
            camera: CameraResponse::from_srgb().expect("the sRGB fallback"),
            backlight,
            options: ForwardOptions {
                samples: spec.samples,
                max_sample_factor: 1,
                threads: spec.threads,
                max_depth: spec.max_depth,
                lambda_range_nm: range,
                bins,
                spectral_sub: sub,
                seed: spec.seed,
                ..ForwardOptions::default()
            },
            views: (0..view_count)
                .map(|v| ViewTraceInput::new(v, grid))
                .collect(),
        }
    }

    /// The input of the forward tracer.
    #[must_use]
    pub fn input(&self) -> ForwardInput<'_> {
        ForwardInput {
            mesh: &self.rough.mesh,
            alignment: Rigid::IDENTITY,
            rig: &self.rig,
            surfaces: &self.rough.surfaces,
            index: &self.index,
            zones: self.truth.geometry.as_ref(),
            inclusions: &[],
            camera: &self.camera,
            backlight: &self.backlight,
            views: &self.views,
            options: &self.options,
            cache_dir: None,
        }
    }

    /// The scene of the `locate` module (for marks, overlays and suggestions).
    #[must_use]
    pub const fn scene(&self) -> Scene<'_> {
        Scene::new(&self.rough.mesh, &self.rig.rig, Rigid::IDENTITY)
    }

    /// Traces the rig.
    ///
    /// # Panics
    ///
    /// When the trace fails.
    #[must_use]
    pub fn trace(&self) -> ForwardRecords {
        trace_rig(&self.input(), &AtomicBool::new(false), &mut |_| {}).expect("the trace runs")
    }
}

/// The solver settings that keep the tests quick (the bench uses `FitConfig::default()`). The
/// priors, the host and the common gain are the SHIPPED defaults: model B alone (`host_id`
/// `None`, owner decision 2; a chromophore model next to it would double the solve time and could
/// be the one that is reported), the Gaussian-process smoothness and L1 magnitude priors of
/// round 5, the gain split of round 2 and `fix_common_gain` (the white frames fix the common
/// exposure, ZC-5). A test must not fit with a setting the application does not use.
#[must_use]
pub fn quick_fit_config(threads: usize) -> FitConfig {
    FitConfig {
        host_id: None,
        seeds: 4,
        stage1_iterations: 10,
        finalists: 1,
        threads,
        ..FitConfig::default()
    }
}

/// The natural logarithm of the synthetic photos' per-view gains, in the order of the records.
#[must_use]
pub fn truth_log_gains(views: usize) -> Vec<f64> {
    (0..views)
        .map(|slot| GAINS[slot % GAINS.len()].ln())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Photos from the forward model
// ---------------------------------------------------------------------------------------------

/// Photos of `records` for the truth's absorption: the prediction times the per-view gain plus
/// seeded Gaussian noise of `noise`, with that noise as the stated variance.
#[must_use]
pub fn photos_forward(
    records: &ForwardRecords,
    truth: &Truth,
    noise: &NoiseSpec,
    seed: u64,
) -> Vec<ObservedView> {
    let alpha = |zone: usize, lambda: f64| truth.alpha(zone, lambda);
    evaluate(records, &alpha)
        .iter()
        .enumerate()
        .map(|(slot, prediction)| {
            let gain = GAINS[slot % GAINS.len()];
            let mut rng =
                SplitMix::new(seed.wrapping_mul(0x1000_0001).wrapping_add(slot as u64 + 1));
            let n = prediction.rgb.len();
            let mut values = vec![[0.0_f32; 3]; n];
            let mut variance = vec![[1e-6_f32; 3]; n];
            for p in 0..n {
                if !prediction.valid[p] {
                    continue;
                }
                for c in 0..3 {
                    let clean = gain * f64::from(prediction.rgb[p][c]);
                    let var = noise.transmittance_variance(clean).max(1e-9);
                    values[p][c] = f32_of(var.sqrt().mul_add(rng.gauss(), clean));
                    variance[p][c] = f32_of(var);
                }
            }
            ObservedView {
                view: prediction.view,
                width: prediction.grid.width,
                height: prediction.grid.height,
                values,
                variance,
            }
        })
        .collect()
}

/// An 8-bit sRGB JPEG of linear pixels.
fn encode_jpeg(width: usize, height: usize, pixels: &[[f32; 3]], quality: u8) -> Vec<u8> {
    let mut raw = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        for &channel in pixel {
            raw.push(byte_of(
                (linear_to_srgb(channel.clamp(0.0, 1.0)) * 255.0).round(),
            ));
        }
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode(&raw, u32_of(width), u32_of(height), ExtendedColorType::Rgb8)
        .expect("the JPEG encodes");
    bytes
}

/// The JPEG round trip of a linear picture: encoded as an 8-bit sRGB JPEG of `quality` and
/// decoded through the photometry lane's [`decode_standard_bytes`]. A flat field must come back
/// within the 8-bit quantisation (the generator's own check).
///
/// # Panics
///
/// When the codec fails.
#[must_use]
pub fn jpeg_round_trip(
    width: usize,
    height: usize,
    pixels: &[[f32; 3]],
    quality: u8,
) -> LinearImage {
    decode_standard_bytes(&encode_jpeg(width, height, pixels, quality)).expect("the JPEG decodes")
}

/// Fills the pixels with `valid == false` from their valid neighbours (repeated four-neighbour
/// averaging, deterministic). The pixels outside the stone carry no information, but a JPEG block
/// transform spreads a bright background into the stone pixels next to it (ringing and chroma
/// bleeding), which the real photos have too and the edge masks remove; the synthetic frame is
/// extended smoothly instead so that the round trip tests the codec, not the background.
fn extend_into_invalid(width: usize, height: usize, values: &mut [[f64; 3]], valid: &[bool]) {
    let mut known = valid.to_vec();
    for _ in 0..(width + height) {
        if known.iter().all(|k| *k) {
            break;
        }
        let snapshot = known.clone();
        let before = values.to_vec();
        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                if snapshot[index] {
                    continue;
                }
                let mut sum = [0.0_f64; 3];
                let mut count = 0.0;
                for (dx, dy) in [(-1_isize, 0_isize), (1, 0), (0, -1), (0, 1)] {
                    let (Some(nx), Some(ny)) = (x.checked_add_signed(dx), y.checked_add_signed(dy))
                    else {
                        continue;
                    };
                    if nx >= width || ny >= height {
                        continue;
                    }
                    let near = ny * width + nx;
                    if snapshot[near] {
                        for (acc, value) in sum.iter_mut().zip(&before[near]) {
                            *acc += value;
                        }
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    for (value, total) in values[index].iter_mut().zip(&sum) {
                        *value = total / count;
                    }
                    known[index] = true;
                }
            }
        }
    }
    // A view with no valid pixel at all: a neutral frame.
    for (value, k) in values.iter_mut().zip(&known) {
        if !*k {
            *value = [1.0; 3];
        }
    }
}

/// The JPEG variant of [`photos_forward`], run through the photometry lane's own pipeline (S2).
///
/// The JPEG block-DC error is a property of the photo's 8 x 8 block grid, so the photo is built at
/// [`JPEG_BLOCK`] times the working resolution: every working pixel is one aligned 8 x 8 block of
/// photo pixels whose cell mean is the prediction (white level times the prediction times the
/// gain, the pixels outside the stone extended from their neighbours), reconstructed piecewise
/// linearly from the cell-mean slopes so that the photo is smooth like a real one. Each photo
/// pixel carries the sensor noise of `noise` scaled by `JPEG_BLOCK^2` (so a working pixel keeps
/// the stated noise after the average). The stone frame and a white frame at [`WHITE_LEVEL`] with
/// the same sensor noise are encoded as 8-bit sRGB JPEG, decoded through
/// [`decode_standard_bytes`], calibrated with `ViewCalibration::transmittance_masked` (outside
/// pixels masked, the compression terms estimated from the frames: per-pixel and block-DC) and
/// area-averaged onto the working grid with `TransmittanceImage::to_working`. The stated variance
/// is the one that pipeline propagates, so it is by construction what the application states for
/// such a photo.
///
/// # Panics
///
/// When the codec fails, the decoded frames differ in size, or the calibration is refused.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one pipeline (prediction, JPEG frames, calibration, working grid) read top to bottom"
)]
pub fn photos_jpeg(
    records: &ForwardRecords,
    truth: &Truth,
    noise: &NoiseSpec,
    seed: u64,
    quality: u8,
) -> Vec<ObservedView> {
    let alpha = |zone: usize, lambda: f64| truth.alpha(zone, lambda);
    evaluate(records, &alpha)
        .iter()
        .enumerate()
        .map(|(slot, prediction)| {
            let (width, height) = (prediction.grid.width, prediction.grid.height);
            let gain = GAINS[slot % GAINS.len()];
            let mut rng =
                SplitMix::new(seed.wrapping_mul(0x1000_0001).wrapping_add(slot as u64 + 1));
            let n = prediction.rgb.len();
            let mut transmittance: Vec<[f64; 3]> = (0..n)
                .map(|p| {
                    let r = prediction.rgb[p];
                    [
                        gain * f64::from(r[0]),
                        gain * f64::from(r[1]),
                        gain * f64::from(r[2]),
                    ]
                })
                .collect();
            extend_into_invalid(width, height, &mut transmittance, &prediction.valid);
            // The photo at JPEG_BLOCK times the working resolution. A photo pixel's cell-mean
            // signal is reconstructed piecewise linearly from the central slope of the working
            // pixels around it (zero slope at the edge of the grid): the offsets inside a cell
            // are symmetric, so the cell mean stays the working pixel's value.
            let block = JPEG_BLOCK;
            let (photo_w, photo_h) = (width * block, height * block);
            let photo_signal = |x: usize, y: usize, c: usize| -> f64 {
                let (px, py) = (x / block, y / block);
                let at = |qx: usize, qy: usize| transmittance[qy * width + qx][c];
                let slope_x = if px == 0 || px + 1 == width {
                    0.0
                } else {
                    0.5 * (at(px + 1, py) - at(px - 1, py))
                };
                let slope_y = if py == 0 || py + 1 == height {
                    0.0
                } else {
                    0.5 * (at(px, py + 1) - at(px, py - 1))
                };
                let offset = |i: usize| ((i % block) as f64 + 0.5) / block as f64 - 0.5;
                WHITE_LEVEL * slope_y.mul_add(offset(y), slope_x.mul_add(offset(x), at(px, py)))
            };
            // The sensor noise of the working pixel spreads over the block: per photo pixel the
            // variance is JPEG_BLOCK^2 times the stated one.
            let photo_gain = (block * block) as f64;
            let mut stone = vec![[0.0_f32; 3]; photo_w * photo_h];
            for y in 0..photo_h {
                for x in 0..photo_w {
                    #[expect(
                        clippy::needless_range_loop,
                        reason = "the channel index addresses the pixel's rgb triple and is also an argument of photo_signal"
                    )]
                    for c in 0..3 {
                        let signal = photo_signal(x, y, c);
                        let sampled = (photo_gain * noise.signal_variance(signal))
                            .sqrt()
                            .mul_add(rng.gauss(), signal);
                        stone[y * photo_w + x][c] = f32_of(sampled.clamp(0.0, 1.0));
                    }
                }
            }
            // The white frame carries the sensor noise of `noise` as well (ZC-3): a diffuser frame
            // is nowhere noise-free (shot noise of a 0.7 full-scale frame), and its own
            // stream keeps the stone frame's noise unchanged.
            let mut white_rng = SplitMix::new(
                seed.wrapping_mul(0x2000_0001)
                    .wrapping_add(0x5A17_E000 + slot as u64),
            );
            let white_sigma = (photo_gain * noise.signal_variance(WHITE_LEVEL)).sqrt();
            let white: Vec<[f32; 3]> = (0..photo_w * photo_h)
                .map(|_| {
                    [0; 3].map(|_| {
                        f32_of(
                            white_sigma
                                .mul_add(white_rng.gauss(), WHITE_LEVEL)
                                .clamp(0.0, 1.0),
                        )
                    })
                })
                .collect();
            let stone_back = jpeg_round_trip(photo_w, photo_h, &stone, quality);
            let white_back = jpeg_round_trip(photo_w, photo_h, &white, quality);
            assert_eq!(
                (stone_back.width, stone_back.height, white_back.width),
                (photo_w, photo_h, photo_w),
                "the round trip keeps the size"
            );
            // The photometry lane's own pipeline (ZC-3, S2): the sensor noise per photo pixel is
            // JPEG_BLOCK^2 times the stated one, the pixels outside the stone are masked for the
            // compression estimate, and the transmittance is area-averaged onto the working grid
            // with the per-pixel variance averaged as independent and the block-DC variance as
            // correlated within a block.
            let sensor = NoiseModel {
                a: [f32_of(photo_gain * noise.a); 3],
                b: [f32_of(photo_gain * noise.b); 3],
            };
            let mut outside = PixelMask::new(photo_w, photo_h);
            for y in 0..photo_h {
                for x in 0..photo_w {
                    if !prediction.valid[(y / block) * width + x / block] {
                        outside.set(x, y, flag::OUTSIDE_OUTLINE);
                    }
                }
            }
            let calibration = ViewCalibration {
                frames: CalibrationFrames {
                    white: vec![white_back],
                    dark: Vec::new(),
                },
                noise: sensor,
            };
            let photo = calibration
                .transmittance_masked(
                    &stone_back,
                    &TransmittanceOptions::default(),
                    Some(&outside),
                )
                .expect("the stone and white frames agree in size");
            let grid = WorkingGrid {
                width,
                height,
                origin: [0.0, 0.0],
                scale: block as f64,
            };
            let working = photo
                .to_working(&grid, &ResampleOptions::default())
                .expect("the photo covers the working grid");
            let mut values = vec![[0.0_f32; 3]; n];
            let mut variance = vec![[1e-6_f32; 3]; n];
            for p in 0..n {
                for c in 0..3 {
                    if working.variance[p][c].is_finite() {
                        values[p][c] = working.values[p][c];
                        variance[p][c] = working.variance[p][c].max(1e-9);
                    } else {
                        // Nothing usable under the footprint (a saturated or noise-floor block):
                        // the noise-free extended value with the stated sensor noise.
                        let t = transmittance[p][c];
                        values[p][c] = f32_of(t);
                        variance[p][c] = f32_of(noise.transmittance_variance(t).max(1e-9));
                    }
                }
            }
            ObservedView {
                view: prediction.view,
                width,
                height,
                values,
                variance,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Photos from the main CPU tracer (a cube in a uniform white furnace)
// ---------------------------------------------------------------------------------------------

/// Linear sRGB from XYZ (D65), the inverse of the matrix `CameraResponse::from_srgb` is built
/// on, so that this IS the camera of the forward model.
const XYZ_TO_SRGB: [[f64; 3]; 3] = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266_0, 1.876_010_8, 0.041_556_0],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

fn xyz_to_camera_rgb(xyz: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (slot, row) in out.iter_mut().zip(&XYZ_TO_SRGB) {
        for (coefficient, component) in row.iter().zip(&xyz) {
            *slot = coefficient.mul_add(*component, *slot);
        }
    }
    out
}

/// The camera RGB of the empty white furnace: the CMF-weighted spectral reconstruction of the
/// environment radiance [1, 1, 1] (the quantity the main tracer's own white-furnace tests
/// compare a clear stone with).
#[must_use]
pub fn furnace_camera_rgb() -> [f64; 3] {
    let mut xyz = [0.0_f64; 3];
    for step in 0..=(780 - 380) {
        let lambda = 380.0_f32 + step as f32;
        let spec = f64::from(rgb_to_spectral_radiance([1.0, 1.0, 1.0], lambda));
        let cmf = cie_1931_cmf(lambda);
        for (k, acc) in xyz.iter_mut().enumerate() {
            *acc = f64::mul_add(f64::from(cmf[k]), spec, *acc);
        }
    }
    let integral = f64::from(CIE_1931_Y_INTEGRAL_5NM);
    xyz_to_camera_rgb([xyz[0] / integral, xyz[1] / integral, xyz[2] / integral])
}

/// The camera RGB of the empty white furnace through `camera` (the response the forward model
/// uses): the trapezoid integral of the camera sensitivity times the environment's spectral
/// radiance, over the Y integral of the CIE table (the normalisation of the main tracer's XYZ).
/// For the sRGB fallback response this is the same quantity as [`furnace_camera_rgb`] up to the
/// quadrature (5 nm trapezoid against 1 nm sum).
#[must_use]
pub fn camera_furnace_rgb(camera: &CameraResponse) -> [f64; 3] {
    let rgb = camera
        .camera_rgb(&|lambda| f64::from(rgb_to_spectral_radiance([1.0, 1.0, 1.0], f32_of(lambda))));
    let integral = f64::from(CIE_1931_Y_INTEGRAL_5NM);
    [rgb[0] / integral, rgb[1] / integral, rgb[2] / integral]
}

/// The bounce limit of the main tracer, and of the forward model when it is compared with it
/// (round 3, D5): both trace the same number of bounces, so a pale stone, whose trapped light
/// bounces long, does not lose different shares of its weight at the depth limit.
pub const MAIN_TRACER_DEPTH: u32 = 64;

/// The six planes of the cube with half edge `half` model units (`n . x + d <= 0` inside).
#[must_use]
pub fn cube_planes(half: f32) -> Vec<GpuFacetPlane> {
    let mut planes = Vec::with_capacity(6);
    for axis in 0..3 {
        for sign in [-1.0_f32, 1.0] {
            let mut normal = Vec3::ZERO;
            normal[axis] = sign;
            planes.push(GpuFacetPlane::new(normal, -half));
        }
    }
    planes
}

/// The truth as a material of the main tracer: a non-dispersive stone of [`STONE_N`] with the
/// zoned absorption (a uniform stone is a zoned one without shaped zones), per mm, with
/// [`MM_PER_MODEL_UNIT`] millimetres per model unit.
#[must_use]
pub fn main_material(truth: &Truth) -> GemMaterial {
    let zoning = truth
        .geometry
        .clone()
        .unwrap_or_else(|| ZonedAbsorption::new(truth.zone(0).clone()));
    GemMaterial::new_custom(
        "validation stone",
        f32_of(STONE_N),
        0.0,
        0.0,
        [0.0, 0.0, 0.0],
    )
    .with_zoning(zoning)
    .with_absorption_path_scale(MM_PER_MODEL_UNIT)
}

/// The main tracer's image of one view on the working grid: the camera RGB relative to the empty
/// furnace, its mean over the samples and the variance of that mean.
#[derive(Debug, Clone)]
pub struct MainView {
    pub view: usize,
    pub width: usize,
    pub height: usize,
    pub mean: Vec<[f64; 3]>,
    pub variance_of_mean: Vec<[f64; 3]>,
}

impl MainView {
    /// The view as solver input (the variance floored).
    #[must_use]
    pub fn observed(&self) -> ObservedView {
        ObservedView {
            view: self.view,
            width: self.width,
            height: self.height,
            values: self
                .mean
                .iter()
                .map(|m| [f32_of(m[0]), f32_of(m[1]), f32_of(m[2])])
                .collect(),
            variance: self
                .variance_of_mean
                .iter()
                .map(|v| {
                    [
                        f32_of(v[0].max(1e-6)),
                        f32_of(v[1].max(1e-6)),
                        f32_of(v[2].max(1e-6)),
                    ]
                })
                .collect(),
        }
    }
}

/// Renders the cube of `setup` with the main CPU tracer: for every working pixel of every view
/// `spp` rays spread over the pixel footprint (as the forward model does) of the orthographic
/// camera, in a uniform white furnace.
///
/// # Panics
///
/// When the setup is not the unit cube (`scale` 1) of the main tracer's reach.
#[must_use]
pub fn render_main(setup: &Setup, spp: u32, salt: u32) -> Vec<MainView> {
    assert!(
        setup.rough.kind.main_tracer_can_render() && (setup.spec.scale - 1.0).abs() < 1e-12,
        "the main tracer renders the unit cube only"
    );
    let half_units = 5.0 / MM_PER_MODEL_UNIT;
    let planes = cube_planes(half_units);
    let material = main_material(&setup.truth);
    let environment = EnvironmentMap::uniform(64, 32, [1.0, 1.0, 1.0]);
    // The reference is the empty furnace seen through the SAME camera response the forward
    // model weighs its wavelengths with (round 2, C5), not a separate CMF reconstruction.
    let reference = camera_furnace_rgb(&setup.camera);
    let to_units = f64::from(MM_PER_MODEL_UNIT).recip();
    let mut out = Vec::with_capacity(setup.views.len());
    for (slot, view) in setup.views.iter().enumerate() {
        let pose = &setup.rig.rig.views[view.view];
        let (width, height) = (view.grid.width, view.grid.height);
        let mut mean = vec![[0.0_f64; 3]; width * height];
        let mut variance_of_mean = vec![[0.0_f64; 3]; width * height];
        for y in 0..height {
            for x in 0..width {
                let pixel = y * width + x;
                // The forward model averages over the whole working-pixel footprint, so the main
                // tracer must as well: a centre ray alone sees a zone boundary that cuts a pixel
                // as all-or-nothing (a 1.7 percent bias of the bicolour cross-check). R2
                // low-discrepancy points with a per-pixel shift, deterministic.
                let [u0, v0, u1, v1] = view.grid.footprint(x, y);
                let pixel_id = u32_of(slot * width * height + pixel);
                let shift_x = f64::from(hash_u32(pixel_id ^ 0x9E37_79B9)) / 4_294_967_296.0;
                let shift_y = f64::from(hash_u32(pixel_id ^ 0x85EB_CA6B)) / 4_294_967_296.0;
                let (mut sum, mut sum_sq) = ([0.0_f64; 3], [0.0_f64; 3]);
                for s in 0..spp {
                    let draw = f64::from(s) + 1.0;
                    let fx = 0.754_877_666_246_692_7f64.mul_add(draw, shift_x).fract();
                    let fy = 0.569_840_290_998_053_3f64.mul_add(draw, shift_y).fract();
                    let at = DVec2::new(fx.mul_add(u1 - u0, u0), fy.mul_add(v1 - v0, v0));
                    let (origin, direction) = pose.pixel_ray(at);
                    let ray = Ray {
                        origin: (origin * to_units).as_vec3(),
                        dir: direction.as_vec3(),
                    };
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ salt));
                    let hero = (hash_u32(seed) as f32) / 4_294_967_295.0;
                    let xyz = trace_spectral_ray(
                        ray,
                        &planes,
                        &material,
                        MAIN_TRACER_DEPTH,
                        EnvironmentSource::HdrMap(&environment),
                        seed,
                        hero,
                        None,
                    );
                    let rgb =
                        xyz_to_camera_rgb([f64::from(xyz.x), f64::from(xyz.y), f64::from(xyz.z)]);
                    for c in 0..3 {
                        let relative = rgb[c] / reference[c];
                        sum[c] += relative;
                        sum_sq[c] = relative.mul_add(relative, sum_sq[c]);
                    }
                }
                let spp_f = f64::from(spp);
                for c in 0..3 {
                    let m = sum[c] / spp_f;
                    mean[pixel][c] = m;
                    let sample_var = f64::mul_add(m, -m, sum_sq[c] / spp_f).max(0.0) * spp_f
                        / (spp_f - 1.0).max(1.0);
                    variance_of_mean[pixel][c] = sample_var / spp_f;
                }
            }
        }
        out.push(MainView {
            view: view.view,
            width,
            height,
            mean,
            variance_of_mean,
        });
    }
    out
}

/// The mean relative radiance of a stone in the white furnace, once by the forward model and
/// once by the main CPU tracer, over the same pixels (those the forward model keeps).
#[derive(Debug, Clone, Copy)]
pub struct Agreement {
    pub forward: f64,
    pub main: f64,
    /// The Monte-Carlo standard error of `forward` (round 5, ZC-15): the records' relative error
    /// of the clear-stone transmittance (the sum of the weights, which the adaptive sampling
    /// converges on) times the prediction. The first version used the error at 0.3 per mm, which
    /// is 5 to 10 times the real figure for a pale stone and made a genuine bias look like noise.
    /// For an absorbing stone the relative error grows with the optical depth, so this is a
    /// lower bound: a z-score computed with it can only overstate a bias, never hide one.
    pub forward_se: f64,
    /// The Monte-Carlo standard error of `main`: the root of the summed per-pixel variances of
    /// the mean, over the number of pixels.
    pub main_se: f64,
}

impl Agreement {
    /// `|forward - main| / main`.
    #[must_use]
    pub fn relative_difference(&self) -> f64 {
        (self.forward - self.main).abs() / self.main.abs().max(1e-12)
    }

    /// `|forward - main|` in units of the combined standard error: within 2 the difference is
    /// compatible with Monte-Carlo noise (raise the samples), beyond 2 it is a bias.
    #[must_use]
    pub fn z_score(&self) -> f64 {
        (self.forward - self.main).abs() / self.forward_se.hypot(self.main_se).max(1e-12)
    }
}

/// The furnace cross-check of the unit cube for `truth` (three views, 10 x 10 working pixels).
#[must_use]
pub fn furnace_agreement(truth: &Truth, forward_samples: usize, main_spp: u32) -> Agreement {
    let channels = furnace_agreement_rgb(truth, forward_samples, main_spp);
    let mean = |f: fn(&Agreement) -> f64| channels.iter().map(f).sum::<f64>() / 3.0;
    let error =
        |f: fn(&Agreement) -> f64| channels.iter().map(|a| f(a) * f(a)).sum::<f64>().sqrt() / 3.0;
    Agreement {
        forward: mean(|a| a.forward),
        main: mean(|a| a.main),
        forward_se: error(|a| a.forward_se),
        main_se: error(|a| a.main_se),
    }
}

/// [`furnace_agreement`] per camera channel (red, green, blue): a calibration offset that depends
/// on the colour of the stone (the camera response, the light spectrum, the wavelength grid) shows
/// here while the channel mean may still agree.
#[must_use]
pub fn furnace_agreement_rgb(
    truth: &Truth,
    forward_samples: usize,
    main_spp: u32,
) -> [Agreement; 3] {
    let spec = SetupSpec {
        rough: RoughKind::Cube,
        views: 3,
        grid_px: 10,
        samples: forward_samples,
        light: Light::Surround,
        spectrum: Spectrum::EnvironmentWhite,
        max_depth: MAIN_TRACER_DEPTH as usize,
        ..SetupSpec::default()
    };
    let setup = Setup::with_truth(&spec, truth.clone());
    let records = setup.trace();
    let alpha = |zone: usize, lambda: f64| truth.alpha(zone, lambda);
    let predictions = evaluate(&records, &alpha);
    let main = render_main(&setup, main_spp, 0xA11C_E000);
    let (mut sum_forward, mut sum_main, mut count) = ([0.0_f64; 3], [0.0_f64; 3], 0_usize);
    let (mut var_forward, mut var_main) = ([0.0_f64; 3], [0.0_f64; 3]);
    for (slot, pred) in predictions.iter().enumerate() {
        let view = &records.views[slot];
        for p in 0..pred.rgb.len() {
            if !pred.valid[p] {
                continue;
            }
            let mc_mean = f64::from(view.mc_mean_clear[p]);
            let relative = if mc_mean > 1e-6 {
                f64::from(view.mc_variance_clear[p]).sqrt() / mc_mean
            } else {
                0.0
            };
            for c in 0..3 {
                sum_forward[c] += f64::from(pred.rgb[p][c]);
                sum_main[c] += main[slot].mean[p][c];
                let forward_error = relative * f64::from(pred.rgb[p][c]);
                var_forward[c] = f64::mul_add(forward_error, forward_error, var_forward[c]);
                var_main[c] += main[slot].variance_of_mean[p][c];
            }
            count += 1;
        }
    }
    assert!(count > 0, "the forward model kept no pixel");
    let n = count as f64;
    [0, 1, 2].map(|c| Agreement {
        forward: sum_forward[c] / n,
        main: sum_main[c] / n,
        forward_se: var_forward[c].sqrt() / n,
        main_se: var_main[c].sqrt() / n,
    })
}

// ---------------------------------------------------------------------------------------------
// Boundary error metrics
// ---------------------------------------------------------------------------------------------

/// The largest distance, over a grid of points of the TRUE plane `normal . p = offset` inside
/// the stone (`extent_mm` half width), from the fitted half space. Millimetres.
#[must_use]
pub fn plane_error_mm(
    fitted_normal: DVec3,
    fitted_offset: f64,
    true_normal: DVec3,
    true_offset: f64,
    extent_mm: f64,
) -> f64 {
    let normal = true_normal.normalize();
    let axis_a = normal.any_orthonormal_vector();
    let axis_b = normal.cross(axis_a);
    let fitted = fitted_normal.normalize();
    let mut worst = 0.0_f64;
    for i in -2..=2 {
        for j in -2..=2 {
            let point = normal * true_offset
                + axis_a * (extent_mm * f64::from(i) / 2.0)
                + axis_b * (extent_mm * f64::from(j) / 2.0);
            worst = worst.max((fitted.dot(point) - fitted_offset).abs());
        }
    }
    worst
}

/// The largest distance of points of the three true faces of the trigonal prism (axis Z,
/// apothem [`WATERMELON_APOTHEM_MM`], phase 0) from the nearest face of the fitted regular
/// prism. `axis_point`, `axis_dir`, `r_out` and `phase` are the fitted parameters, `sides` its
/// side count; the angle reference is the kernels' rule (X, or Y when the axis is within 0.9
/// of X). Millimetres.
#[must_use]
pub fn prism_error_mm(
    axis_point: DVec3,
    axis_dir: DVec3,
    sides: u32,
    r_out: f64,
    phase: f64,
    extent_z_mm: f64,
) -> f64 {
    let axis = axis_dir.normalize();
    let reference = if axis.x.abs() > 0.9 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let tangent_u = (reference - axis * reference.dot(axis)).normalize();
    let tangent_v = axis.cross(tangent_u);
    let fitted_normals: Vec<DVec3> = (0..sides)
        .map(|k| {
            let angle = phase + TAU * f64::from(k) / f64::from(sides);
            tangent_u * angle.cos() + tangent_v * angle.sin()
        })
        .collect();
    let mut worst = 0.0_f64;
    for k in 0..3 {
        let angle = TAU * f64::from(k) / 3.0;
        let normal = DVec3::X * angle.cos() + DVec3::Y * angle.sin();
        let tangent = DVec3::Z.cross(normal);
        for i in -2..=2 {
            for j in -2..=2 {
                let point = normal * WATERMELON_APOTHEM_MM
                    + tangent * (0.5 * f64::from(i))
                    + DVec3::Z * (extent_z_mm * f64::from(j) / 2.0);
                let nearest = fitted_normals
                    .iter()
                    .map(|n| (n.dot(point - axis_point) - r_out).abs())
                    .fold(f64::INFINITY, f64::min);
                worst = worst.max(nearest);
            }
        }
    }
    worst
}

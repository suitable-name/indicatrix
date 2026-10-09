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
//!   "watermelon" prism core, a sector. The spectra are sums of Gaussian bands, which are NOT in
//!   the smooth basis the solver fits (model B), so a recovery test also checks the model error.
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
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::similar_names,
    reason = "a shared fixture module: each test target and the example use a subset, and the \
              small dense loops read better as written"
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
        solve::{ColourFit, FitConfig, ObservedView},
    },
    locate::{Projection, RigProfile, Rigid, Scene, ViewPose, box_mesh},
    photometry::{
        COMPRESSION_SIGMA_ENCODED, LinearImage, NoiseModel, SourceKind, WorkingGrid,
        decode_standard_bytes, encoding_variance_with, estimate_compression_sigma, linear_to_srgb,
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
    let index = vertices.len() as u32;
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
            map = map.with_override(index as u32, SurfaceClass::Polished);
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
            Self::Banded => "banded (4 zones)",
        }
    }
}

/// A zone absorption from `(centre nm, sigma nm, peak per mm)` Gaussian bands.
#[must_use]
pub fn band_zone(bands: &[(f32, f32, f32)]) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(
        bands
            .iter()
            .map(|&(centre, width, peak)| AbsorptionBand::new(centre, width, peak))
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
            ColourCase::UniformPale => (None, Some(band_zone(&[(560.0, 70.0, 0.02)]))),
            ColourCase::UniformSaturated => (
                None,
                Some(band_zone(&[(520.0, 40.0, 0.18), (600.0, 50.0, 0.12)])),
            ),
            ColourCase::Bicolour => (
                zoned(
                    band_zone(&[(450.0, 40.0, 0.08)]),
                    vec![half_space(band_zone(&[(610.0, 40.0, 0.12)]))],
                ),
                None,
            ),
            ColourCase::WatermelonPrism => (
                zoned(
                    band_zone(&[(620.0, 45.0, 0.10)]),
                    vec![Zone {
                        shape: watermelon_prism(),
                        absorption: band_zone(&[(540.0, 40.0, 0.15)]),
                    }],
                ),
                None,
            ),
            ColourCase::Sector => (
                zoned(
                    band_zone(&[(560.0, 60.0, 0.04)]),
                    vec![Zone {
                        shape: sector_wedge(),
                        absorption: band_zone(&[(450.0, 40.0, 0.15)]),
                    }],
                ),
                None,
            ),
            ColourCase::Banded => (
                zoned(
                    band_zone(&[(560.0, 60.0, 0.04)]),
                    vec![
                        half_space(band_zone(&[(610.0, 40.0, 0.10)])),
                        Zone {
                            shape: watermelon_prism(),
                            absorption: band_zone(&[(540.0, 40.0, 0.15)]),
                        },
                        Zone {
                            shape: sector_wedge(),
                            absorption: band_zone(&[(450.0, 40.0, 0.12)]),
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

/// The two accuracy tiers of section 10.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// RAW with a measured sensor response: noise only.
    Raw,
    /// The JPEG fallback: noise plus an 8-bit sRGB JPEG round trip.
    Jpeg,
}

impl Tier {
    /// The CIEDE2000 limit of the predicted face-up colour (1.5 RAW, 3.0 JPEG).
    #[must_use]
    pub const fn face_up_limit(self) -> f64 {
        match self {
            Self::Raw => 1.5,
            Self::Jpeg => 3.0,
        }
    }

    /// The limit of the leave-one-view-out median (2.0 for RAW; the JPEG tier gets 3.0, the
    /// plan states no separate figure).
    #[must_use]
    pub const fn lovo_limit(self) -> f64 {
        match self {
            Self::Raw => 2.0,
            Self::Jpeg => 3.0,
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
    /// A 5000 K LED over 400 to 700 nm in 8 bins.
    Led,
    /// The spectrum of the main tracer's white environment over 380 to 780 nm in 40 bins, so
    /// that forward and main tracer weigh every wavelength alike.
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
        *slot = f64::from(rgb_to_spectral_radiance([1.0, 1.0, 1.0], lambda as f32)).max(0.0);
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
                2,
            ),
            Spectrum::EnvironmentWhite => (
                BacklightSpectrum::from_grid(&environment_white_grid())
                    .expect("the white environment spectrum"),
                [380.0, 780.0],
                40,
                1,
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
/// priors are the SHIPPED defaults (smoothness sigma 0.5, the gain split of round 2): a test must
/// not fit with a prior the application does not use.
#[must_use]
pub fn quick_fit_config(threads: usize) -> FitConfig {
    FitConfig {
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
                    values[p][c] = var.sqrt().mul_add(rng.gauss(), clean) as f32;
                    variance[p][c] = var as f32;
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
        for c in 0..3 {
            raw.push((linear_to_srgb(pixel[c].clamp(0.0, 1.0)) * 255.0).round() as u8);
        }
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode(&raw, width as u32, height as u32, ExtendedColorType::Rgb8)
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
                let i = y * width + x;
                if snapshot[i] {
                    continue;
                }
                let mut sum = [0.0_f64; 3];
                let mut count = 0.0;
                for (dx, dy) in [(-1_i64, 0_i64), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                        continue;
                    }
                    let j = ny as usize * width + nx as usize;
                    if snapshot[j] {
                        for c in 0..3 {
                            sum[c] += before[j][c];
                        }
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    for c in 0..3 {
                        values[i][c] = sum[c] / count;
                    }
                    known[i] = true;
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

/// The JPEG variant of [`photos_forward`]: the stone frame (white level times the prediction
/// times the gain, plus the noise of `noise`, the pixels outside the stone extended from their
/// neighbours) and a flat white frame are encoded as 8-bit sRGB JPEG, decoded through the
/// photometry lane's [`decode_standard_bytes`], and divided (`t = stone / white`, no dark frame).
/// The stated variance is the one the photometry lane gives such a pair: the noise plus the
/// encoding variance ([`encoding_variance_with`], quantisation and compression) of the stone and of
/// the white frame, `(var_s + t^2 var_w) / w^2`.
///
/// # Panics
///
/// When the codec fails or the decoded frames differ in size.
#[must_use]
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
            let mut stone = vec![[0.0_f32; 3]; n];
            for p in 0..n {
                for c in 0..3 {
                    let signal = WHITE_LEVEL * transmittance[p][c];
                    let noisy = noise
                        .signal_variance(signal)
                        .sqrt()
                        .mul_add(rng.gauss(), signal);
                    stone[p][c] = noisy.clamp(0.0, 1.0) as f32;
                }
            }
            let white = vec![[WHITE_LEVEL as f32; 3]; n];
            let stone_back = jpeg_round_trip(width, height, &stone, quality);
            let white_back = jpeg_round_trip(width, height, &white, quality);
            assert_eq!(
                (stone_back.width, stone_back.height, white_back.width),
                (width, height, width),
                "the round trip keeps the size"
            );
            // The compression term the photometry lane estimates from this white frame (round 3,
            // D4.2), the assumed constant when it cannot: the stated variance is the one the lane
            // gives such a pair. A flat white frame compresses almost perfectly, so the estimate
            // is small and the solver's Birge ratio carries the rest.
            let sensor = NoiseModel {
                a: [noise.a as f32; 3],
                b: [noise.b as f32; 3],
            };
            let compression = estimate_compression_sigma(&white_back, &sensor, 0.98)
                .unwrap_or([COMPRESSION_SIGMA_ENCODED; 3]);
            let encoding = |linear: f64, c: usize| {
                f64::from(encoding_variance_with(
                    SourceKind::Rgb8,
                    true,
                    linear as f32,
                    compression[c],
                ))
            };
            let mut values = vec![[0.0_f32; 3]; n];
            let mut variance = vec![[1e-6_f32; 3]; n];
            for p in 0..n {
                for c in 0..3 {
                    let w = f64::from(white_back.pixels[p][c]).max(1e-3);
                    let s = f64::from(stone_back.pixels[p][c]);
                    let t = s / w;
                    values[p][c] = t as f32;
                    let var_s = noise.signal_variance(s) + encoding(s, c);
                    let var_w = noise.signal_variance(w) + encoding(w, c);
                    variance[p][c] = (f64::mul_add(t * t, var_w, var_s) / (w * w)).max(1e-9) as f32;
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
    for i in 0..3 {
        for k in 0..3 {
            out[i] = XYZ_TO_SRGB[i][k].mul_add(xyz[k], out[i]);
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
        for k in 0..3 {
            xyz[k] = f64::mul_add(f64::from(cmf[k]), spec, xyz[k]);
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
        .camera_rgb(&|lambda| f64::from(rgb_to_spectral_radiance([1.0, 1.0, 1.0], lambda as f32)));
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
        STONE_N as f32,
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
                .map(|m| [m[0] as f32, m[1] as f32, m[2] as f32])
                .collect(),
            variance: self
                .variance_of_mean
                .iter()
                .map(|v| {
                    [
                        v[0].max(1e-6) as f32,
                        v[1].max(1e-6) as f32,
                        v[2].max(1e-6) as f32,
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
                let p = y * width + x;
                // The forward model averages over the whole working-pixel footprint, so the main
                // tracer must as well: a centre ray alone sees a zone boundary that cuts a pixel
                // as all-or-nothing (a 1.7 percent bias of the bicolour cross-check). R2
                // low-discrepancy points with a per-pixel shift, deterministic.
                let [u0, v0, u1, v1] = view.grid.footprint(x, y);
                let pixel_id = (slot * width * height + p) as u32;
                let shift_x = f64::from(hash_u32(pixel_id ^ 0x9E37_79B9)) / 4_294_967_296.0;
                let shift_y = f64::from(hash_u32(pixel_id ^ 0x85EB_CA6B)) / 4_294_967_296.0;
                let (mut sum, mut sum_sq) = ([0.0_f64; 3], [0.0_f64; 3]);
                for s in 0..spp {
                    let k = f64::from(s) + 1.0;
                    let fx = 0.754_877_666_246_692_7f64.mul_add(k, shift_x).fract();
                    let fy = 0.569_840_290_998_053_3f64.mul_add(k, shift_y).fract();
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
                let n = f64::from(spp);
                for c in 0..3 {
                    let m = sum[c] / n;
                    mean[p][c] = m;
                    let sample_var =
                        f64::mul_add(m, -m, sum_sq[c] / n).max(0.0) * n / (n - 1.0).max(1.0);
                    variance_of_mean[p][c] = sample_var / n;
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
    /// The Monte-Carlo standard error of `forward` (round 3, D5): the records' relative error at
    /// the reference absorption times the prediction, a conservative figure for a pale stone.
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
    for (slot, prediction) in predictions.iter().enumerate() {
        let view = &records.views[slot];
        for p in 0..prediction.rgb.len() {
            if !prediction.valid[p] {
                continue;
            }
            let mc_mean = f64::from(view.mc_mean[p]);
            let relative = if mc_mean > 1e-6 {
                f64::from(view.mc_variance[p]).sqrt() / mc_mean
            } else {
                0.0
            };
            for c in 0..3 {
                sum_forward[c] += f64::from(prediction.rgb[p][c]);
                sum_main[c] += main[slot].mean[p][c];
                let forward_error = relative * f64::from(prediction.rgb[p][c]);
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
    let n = true_normal.normalize();
    let a = n.any_orthonormal_vector();
    let b = n.cross(a);
    let fitted = fitted_normal.normalize();
    let mut worst = 0.0_f64;
    for i in -2..=2 {
        for j in -2..=2 {
            let p = n * true_offset
                + a * (extent_mm * f64::from(i) / 2.0)
                + b * (extent_mm * f64::from(j) / 2.0);
            worst = worst.max((fitted.dot(p) - fitted_offset).abs());
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
    let u = (reference - axis * reference.dot(axis)).normalize();
    let v = axis.cross(u);
    let fitted_normals: Vec<DVec3> = (0..sides)
        .map(|k| {
            let angle = phase + TAU * f64::from(k) / f64::from(sides);
            u * angle.cos() + v * angle.sin()
        })
        .collect();
    let mut worst = 0.0_f64;
    for k in 0..3 {
        let angle = TAU * f64::from(k) / 3.0;
        let normal = DVec3::X * angle.cos() + DVec3::Y * angle.sin();
        let tangent = DVec3::Z.cross(normal);
        for i in -2..=2 {
            for j in -2..=2 {
                let p = normal * WATERMELON_APOTHEM_MM
                    + tangent * (0.5 * f64::from(i))
                    + DVec3::Z * (extent_z_mm * f64::from(j) / 2.0);
                let nearest = fitted_normals
                    .iter()
                    .map(|n| (n.dot(p - axis_point) - r_out).abs())
                    .fold(f64::INFINITY, f64::min);
                worst = worst.max(nearest);
            }
        }
    }
    worst
}

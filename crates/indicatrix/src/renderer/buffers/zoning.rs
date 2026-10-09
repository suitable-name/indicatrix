//! GPU encoding of zoned absorption (`zoning` feature only): the zone table appended to
//! [`GpuGemMaterial`](super::GpuGemMaterial), and the predicate that decides which zoned
//! materials the GPU kernels can render at all.
//!
//! # What the GPU renders
//!
//! A zoned material (`GemMaterial::zoning == Some`) is rendered on the GPU when
//!
//! * it has no mesh-shell zone (the mesh kernel is f64 and brute force; it exists on the CPU
//!   only),
//! * it has no scattering (`scattering_sigma_s == 0`; the CPU's zoned scattering estimator
//!   inverts a piecewise-linear optical depth by bisection and has no GPU twin),
//! * the zoning is valid ([`ZonedAbsorption::validate`]) and every zone's band sets fit the
//!   fixed [`MAX_ABSORPTION_BANDS`] capacity.
//!
//! Anything else is declined by [`gpu_zoning_decline`], which `GemMaterial::gpu_supported`
//! consults, so the viewer, the worker and the hybrid renderer all route such a material to
//! the CPU tracer. Without this a zoned material on the GPU would silently render its base
//! zone only.
//!
//! # Layout
//!
//! The table is APPENDED to `GpuGemMaterial` at offset 576, so every earlier field keeps its
//! offset (and the `layout_echo.wgsl` prefix echo keeps working). It is built from `u32`,
//! `f32` and 16-byte vectors only; the WGSL twin is `shaders/zoning/01_zone_table.wgsl`, and
//! `renderer::gpu::layout_check::run_zone_table` proves the two byte for byte on a device.
//!
//! * [`GpuZoneHeader`] (80 bytes): `zone_count` (shaped zones; 0 means "unzoned, use the
//!   material's own bands"), `softness`, `soft_subdiv`, the frame `origin` and the three
//!   `rows` of the transposed frame rotation.
//! * [`GpuZoneShape`] (112 bytes) x [`MAX_ZONES`]: one flattened shape, the numbers of
//!   [`ExportedZone`](crate::optics::zoning::ExportedZone).
//! * [`GpuZoneAbsorption`] (400 bytes) x [`MAX_ZONES`]: the band sets of zone `i + 1`.
//!
//! The BASE zone's bands are not in the table: [`GpuGemMaterial::encode`](super::GpuGemMaterial::encode)
//! writes the zoning's base tensor into the material's ordinary `o_ray_bands`/`e_ray_bands`/
//! `beta_ray_bands`, so the shader's per-ray hoisted absorption arrays are the base zone's.

use bytemuck::{Pod, Zeroable};
use core::mem::offset_of;

use super::material::{GpuAbsorptionBand, MAX_ABSORPTION_BANDS, empty_bands, encode_bands};
use crate::optics::{
    absorption::AbsorptionTensor,
    materials::GemMaterial,
    zoning::{MAX_ZONES, SOFT_SUBDIV, ZoneKernelF32, ZoneShape, ZonedAbsorption},
};

/// Gauss-Legendre pieces per boundary band on the GPU (the `SOFT_SUBDIV` of the CPU
/// kernel, `k32`).
///
/// It rides in the table (`GpuZoneHeader::soft_subdiv`), so lowering it is a
/// one-constant change on the host with no shader edit.
///
/// Kept at the CPU value: the soft kernel costs up to roughly `38 * 16 * 8 * 4` depth
/// evaluations per segment in the worst case, which is heavy, but a lower value changes the
/// integration error and nothing here could measure that against the CPU. To trade accuracy
/// for speed, lower it (8, then 4) and re-run the soft-boundary GPU-vs-CPU comparisons
/// (`estimator_check::run_image_comparison_zoned` with `ZonedImageCase::SoftBicolour` and
/// `SoftWatermelon`, reported by the GPU harness group `zoned_absorption`) after each step.
pub const GPU_SOFT_SUBDIV: u32 = SOFT_SUBDIV as u32;

/// `GpuZoneShape::flags` bit: sector span above pi.
pub const ZONE_FLAG_WIDE: u32 = 1;
/// `GpuZoneShape::flags` bit: sector span of a full turn.
pub const ZONE_FLAG_FULL: u32 = 2;

/// The table header; see the module docs.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GpuZoneHeader {
    /// Number of shaped zones in use (0 = the material is unzoned for the shader).
    pub zone_count: u32,
    /// Boundary softness in mm (0 = sharp).
    pub softness: f32,
    /// Gauss-Legendre pieces per boundary band ([`GPU_SOFT_SUBDIV`]).
    pub soft_subdiv: u32,
    _pad: u32,
    /// Zone frame translation, `.w` unused.
    pub origin: [f32; 4],
    /// Rows of the transposed frame rotation, `.w` unused.
    pub rows: [[f32; 4]; 3],
}

/// One flattened zone shape; see the module docs.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GpuZoneShape {
    /// `K_*` shape discriminant (0 = unused slot).
    pub kind: u32,
    /// Prism side count.
    pub n_sides: u32,
    /// [`ZONE_FLAG_WIDE`] | [`ZONE_FLAG_FULL`].
    pub flags: u32,
    _pad: u32,
    /// Axis point, `.w` unused.
    pub p: [f32; 4],
    /// Plane normal or axis direction, `.w` unused.
    pub d: [f32; 4],
    /// Axis reference direction `u`, `.w` unused.
    pub u: [f32; 4],
    /// Axis reference direction `v`, `.w` unused.
    pub v: [f32; 4],
    /// Shape scalars `(x0, x1, x2)`, `.w` unused (see `ExportedZone::x`).
    pub x: [f32; 4],
    /// Sector bounding directions `(e0.x, e0.y, e1.x, e1.y)`.
    pub e: [f32; 4],
}

/// The band sets of one shaped zone, laid out like the material's own.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GpuZoneAbsorption {
    /// Number of `o_ray_bands` in use.
    pub o_ray_band_count: u32,
    /// Number of `e_ray_bands` in use.
    pub e_ray_band_count: u32,
    /// Whether the tensor has a third, `beta`, band set.
    pub has_beta_ray: u32,
    /// Number of `beta_ray_bands` in use.
    pub beta_ray_band_count: u32,
    /// Ordinary / `n_alpha` bands.
    pub o_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    /// Extraordinary / `n_gamma` bands.
    pub e_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    /// `n_beta` bands.
    pub beta_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
}

/// The zone table appended to `GpuGemMaterial`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GpuZoneTable {
    /// Counts, softness and frame.
    pub header: GpuZoneHeader,
    /// The shaped zones' geometry.
    pub shapes: [GpuZoneShape; MAX_ZONES],
    /// The shaped zones' band sets.
    pub absorption: [GpuZoneAbsorption; MAX_ZONES],
}

const _: () = {
    assert!(offset_of!(GpuZoneHeader, zone_count) == 0);
    assert!(offset_of!(GpuZoneHeader, softness) == 4);
    assert!(offset_of!(GpuZoneHeader, soft_subdiv) == 8);
    assert!(offset_of!(GpuZoneHeader, origin) == 16);
    assert!(offset_of!(GpuZoneHeader, rows) == 32);
    assert!(size_of::<GpuZoneHeader>() == 80);

    assert!(offset_of!(GpuZoneShape, kind) == 0);
    assert!(offset_of!(GpuZoneShape, n_sides) == 4);
    assert!(offset_of!(GpuZoneShape, flags) == 8);
    assert!(offset_of!(GpuZoneShape, p) == 16);
    assert!(offset_of!(GpuZoneShape, d) == 32);
    assert!(offset_of!(GpuZoneShape, u) == 48);
    assert!(offset_of!(GpuZoneShape, v) == 64);
    assert!(offset_of!(GpuZoneShape, x) == 80);
    assert!(offset_of!(GpuZoneShape, e) == 96);
    assert!(size_of::<GpuZoneShape>() == 112);

    assert!(offset_of!(GpuZoneAbsorption, o_ray_band_count) == 0);
    assert!(offset_of!(GpuZoneAbsorption, e_ray_band_count) == 4);
    assert!(offset_of!(GpuZoneAbsorption, has_beta_ray) == 8);
    assert!(offset_of!(GpuZoneAbsorption, beta_ray_band_count) == 12);
    assert!(offset_of!(GpuZoneAbsorption, o_ray_bands) == 16);
    assert!(offset_of!(GpuZoneAbsorption, e_ray_bands) == 144);
    assert!(offset_of!(GpuZoneAbsorption, beta_ray_bands) == 272);
    assert!(size_of::<GpuZoneAbsorption>() == 400);

    assert!(offset_of!(GpuZoneTable, header) == 0);
    assert!(offset_of!(GpuZoneTable, shapes) == 80);
    assert!(offset_of!(GpuZoneTable, absorption) == 528);
    assert!(size_of::<GpuZoneTable>() == 2128);
    // The WGSL struct alignment of the table is 16: its size must be a multiple of that.
    assert!(size_of::<GpuZoneTable>().is_multiple_of(16));
};

/// Why a zoned material is not rendered on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuZoneDecline {
    /// A zone is a freeform mesh shell (CPU only).
    MeshShell,
    /// The material scatters; zoned scattering is CPU only.
    Scattering,
    /// [`ZonedAbsorption::validate`] failed.
    Invalid,
    /// A band set has more than [`MAX_ABSORPTION_BANDS`] bands (the GPU would truncate it).
    TooManyBands,
}

impl core::fmt::Display for GpuZoneDecline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::MeshShell => "a mesh-shell zone is CPU only",
            Self::Scattering => "a zoned scattering stone is CPU only",
            Self::Invalid => "the zoning is invalid",
            Self::TooManyBands => "a zone has more absorption bands than the GPU holds",
        })
    }
}

impl std::error::Error for GpuZoneDecline {}

fn tensor_fits(tensor: &AbsorptionTensor) -> bool {
    tensor.o_ray.len() <= MAX_ABSORPTION_BANDS
        && tensor.e_ray.len() <= MAX_ABSORPTION_BANDS
        && tensor
            .beta_ray
            .as_ref()
            .is_none_or(|b| b.len() <= MAX_ABSORPTION_BANDS)
}

const fn pad4(v: [f32; 3]) -> [f32; 4] {
    [v[0], v[1], v[2], 0.0]
}

fn encode_absorption(tensor: &AbsorptionTensor) -> GpuZoneAbsorption {
    let (o_ray_bands, o_ray_band_count) = encode_bands(&tensor.o_ray);
    let (e_ray_bands, e_ray_band_count) = encode_bands(&tensor.e_ray);
    let (beta_ray_bands, beta_ray_band_count) = tensor
        .beta_ray
        .as_deref()
        .map_or_else(empty_bands, encode_bands);
    GpuZoneAbsorption {
        o_ray_band_count,
        e_ray_band_count,
        has_beta_ray: u32::from(tensor.beta_ray.is_some()),
        beta_ray_band_count,
        o_ray_bands,
        e_ray_bands,
        beta_ray_bands,
    }
}

impl GpuZoneTable {
    /// Encodes `zoning`. The kernel data come from the f32 CPU kernel's own flattening
    /// ([`ZoneKernelF32::export`]), so both kernels start from the same numbers.
    ///
    /// # Errors
    ///
    /// [`GpuZoneDecline`] if the GPU cannot render these zones.
    pub fn encode(zoning: &ZonedAbsorption) -> Result<Self, GpuZoneDecline> {
        if zoning
            .zones
            .iter()
            .any(|zone| matches!(zone.shape, ZoneShape::MeshShell { .. }))
        {
            return Err(GpuZoneDecline::MeshShell);
        }
        if zoning.validate().is_err() {
            return Err(GpuZoneDecline::Invalid);
        }
        if !tensor_fits(&zoning.base.tensor)
            || zoning
                .zones
                .iter()
                .any(|z| !tensor_fits(&z.absorption.tensor))
        {
            return Err(GpuZoneDecline::TooManyBands);
        }
        let kernel = ZoneKernelF32::new(zoning).ok_or(GpuZoneDecline::MeshShell)?;
        let ex = kernel.export();

        let mut table = Self::zeroed();
        table.header = GpuZoneHeader {
            zone_count: ex.count as u32,
            softness: ex.softness,
            soft_subdiv: GPU_SOFT_SUBDIV,
            _pad: 0,
            origin: pad4(ex.origin),
            rows: [pad4(ex.rows[0]), pad4(ex.rows[1]), pad4(ex.rows[2])],
        };
        for (i, zone) in ex.zones.iter().enumerate().take(ex.count) {
            table.shapes[i] = GpuZoneShape {
                kind: zone.kind,
                n_sides: zone.n_sides,
                flags: (if zone.wide { ZONE_FLAG_WIDE } else { 0 })
                    | (if zone.full { ZONE_FLAG_FULL } else { 0 }),
                _pad: 0,
                p: pad4(zone.p),
                d: pad4(zone.d),
                u: pad4(zone.u),
                v: pad4(zone.v),
                x: pad4(zone.x),
                e: [zone.e0[0], zone.e0[1], zone.e1[0], zone.e1[1]],
            };
            table.absorption[i] = encode_absorption(&zoning.zones[i].absorption.tensor);
        }
        Ok(table)
    }
}

/// Why `material` cannot be rendered on the GPU because of its zones, or `None` when it has
/// no zones or the GPU can render them. `GemMaterial::gpu_supported` is `true` exactly when
/// this is `None`.
#[must_use]
pub fn gpu_zoning_decline(material: &GemMaterial) -> Option<GpuZoneDecline> {
    let zoning = material.zoning.as_ref()?;
    if material.scattering_sigma_s > 0.0 {
        return Some(GpuZoneDecline::Scattering);
    }
    GpuZoneTable::encode(zoning).err()
}

/// The absorption tensor and zone table a material is uploaded with: for a renderable zoned
/// material the zoning's BASE tensor (so the shader's hoisted per-ray arrays are the base
/// zone's, whatever `GemMaterial::absorption` holds) and the encoded table; otherwise the
/// material's own tensor and an all-zero table (`zone_count == 0`, i.e. unzoned).
pub(super) fn material_parts(material: &GemMaterial) -> (&AbsorptionTensor, GpuZoneTable) {
    material
        .zoning
        .as_ref()
        .filter(|_| material.scattering_sigma_s <= 0.0)
        .and_then(|zoning| {
            GpuZoneTable::encode(zoning)
                .ok()
                .map(|table| (&zoning.base.tensor, table))
        })
        .unwrap_or_else(|| (&material.absorption, GpuZoneTable::zeroed()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::{
        absorption::AbsorptionBand,
        zoning::{
            K_CYL, K_HALF, K_NONE, K_PRISM, K_SECTOR, K_SLAB, Zone, ZoneAbsorption, ZoneFrame,
        },
    };
    use glam::DVec3;

    const TABLE_WGSL: &str = include_str!("../shaders/zoning/01_zone_table.wgsl");

    fn tensor(peak: f32) -> AbsorptionTensor {
        AbsorptionTensor::isotropic(vec![AbsorptionBand::new(560.0, 60.0, peak)])
    }

    fn half_space(normal: DVec3, offset: f64, peak: f32) -> Zone {
        Zone {
            shape: ZoneShape::HalfSpace { normal, offset },
            absorption: ZoneAbsorption::per_mm(tensor(peak)),
        }
    }

    fn zoning_with(zones: Vec<Zone>) -> ZonedAbsorption {
        ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(tensor(0.05)),
            zones,
            boundary_softness_mm: 0.0,
        }
    }

    /// Reads `const NAME: u32 = Nu;` out of the zone-table WGSL.
    fn wgsl_const(name: &str) -> u32 {
        let needle = format!("const {name}: u32 = ");
        let start = TABLE_WGSL
            .find(&needle)
            .unwrap_or_else(|| panic!("{name} missing from 01_zone_table.wgsl"))
            + needle.len();
        let end = TABLE_WGSL[start..].find("u;").expect("u32 literal") + start;
        TABLE_WGSL[start..end]
            .trim()
            .parse()
            .expect("decimal literal")
    }

    #[test]
    fn the_wgsl_shape_constants_equal_the_kernel_discriminants() {
        assert_eq!(wgsl_const("ZONE_KIND_NONE"), K_NONE);
        assert_eq!(wgsl_const("ZONE_KIND_HALF"), K_HALF);
        assert_eq!(wgsl_const("ZONE_KIND_SLAB"), K_SLAB);
        assert_eq!(wgsl_const("ZONE_KIND_CYL"), K_CYL);
        assert_eq!(wgsl_const("ZONE_KIND_PRISM"), K_PRISM);
        assert_eq!(wgsl_const("ZONE_KIND_SECTOR"), K_SECTOR);
        assert_eq!(wgsl_const("ZONE_FLAG_WIDE"), ZONE_FLAG_WIDE);
        assert_eq!(wgsl_const("ZONE_FLAG_FULL"), ZONE_FLAG_FULL);
    }

    #[test]
    fn an_unzoned_material_gets_an_all_zero_table_and_its_own_bands() {
        let material = GemMaterial::diamond();
        let (absorption, table) = material_parts(&material);
        assert_eq!(absorption, &material.absorption);
        assert_eq!(table.header.zone_count, 0);
        assert!(bytemuck::bytes_of(&table).iter().all(|&b| b == 0));
        assert_eq!(gpu_zoning_decline(&material), None);
    }

    #[test]
    fn the_table_carries_the_kernel_numbers_and_the_zone_bands() {
        let zoning = zoning_with(vec![
            half_space(DVec3::X, 0.25, 0.4),
            half_space(DVec3::Y, -0.5, 0.7),
        ]);
        let table = GpuZoneTable::encode(&zoning).expect("encodable");
        let ex = ZoneKernelF32::new(&zoning).expect("no mesh").export();
        assert_eq!(table.header.zone_count, 2);
        assert_eq!(table.header.soft_subdiv, GPU_SOFT_SUBDIV);
        assert_eq!(table.header.softness.to_bits(), ex.softness.to_bits());
        assert_eq!(table.shapes[0].kind, K_HALF);
        assert_eq!(table.shapes[0].d[..3], ex.zones[0].d);
        assert_eq!(table.shapes[0].x[0].to_bits(), 0.25f32.to_bits());
        assert_eq!(table.shapes[1].x[0].to_bits(), (-0.5f32).to_bits());
        assert_eq!(table.shapes[2].kind, K_NONE);
        assert_eq!(table.absorption[0].o_ray_band_count, 1);
        assert_eq!(
            table.absorption[0].o_ray_bands[0].peak.to_bits(),
            0.4f32.to_bits()
        );
        assert_eq!(
            table.absorption[1].o_ray_bands[0].peak.to_bits(),
            0.7f32.to_bits()
        );
        assert_eq!(table.absorption[2].o_ray_band_count, 0);
    }

    #[test]
    fn a_zoned_material_uploads_the_zoning_base_not_its_own_absorption() {
        let mut material =
            GemMaterial::diamond().with_zoning(zoning_with(vec![half_space(DVec3::X, 0.0, 0.4)]));
        // Make the plain tensor disagree with the base zone: the GPU must follow the zones.
        material.absorption = tensor(9.0);
        let (absorption, table) = material_parts(&material);
        assert_eq!(absorption, &tensor(0.05));
        assert_eq!(table.header.zone_count, 1);
    }

    #[test]
    fn a_mesh_shell_scattering_or_too_many_bands_decline() {
        let mesh = Zone {
            shape: ZoneShape::MeshShell {
                vertices: vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                ],
                triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            },
            absorption: ZoneAbsorption::per_mm(tensor(0.3)),
        };
        let with_mesh = GemMaterial::diamond().with_zoning(zoning_with(vec![mesh]));
        assert_eq!(
            gpu_zoning_decline(&with_mesh),
            Some(GpuZoneDecline::MeshShell)
        );

        let mut scattering =
            GemMaterial::diamond().with_zoning(zoning_with(vec![half_space(DVec3::X, 0.0, 0.4)]));
        scattering.scattering_sigma_s = 0.5;
        assert_eq!(
            gpu_zoning_decline(&scattering),
            Some(GpuZoneDecline::Scattering)
        );

        let many = AbsorptionTensor::isotropic(
            (0..=MAX_ABSORPTION_BANDS)
                .map(|i| AbsorptionBand::new(10.0f32.mul_add(i as f32, 450.0), 20.0, 0.1))
                .collect(),
        );
        let mut zoning = zoning_with(vec![half_space(DVec3::X, 0.0, 0.4)]);
        zoning.zones[0].absorption = ZoneAbsorption::per_mm(many);
        let too_many = GemMaterial::diamond().with_zoning(zoning);
        assert_eq!(
            gpu_zoning_decline(&too_many),
            Some(GpuZoneDecline::TooManyBands)
        );

        let fine =
            GemMaterial::diamond().with_zoning(zoning_with(vec![half_space(DVec3::X, 0.0, 0.4)]));
        assert_eq!(gpu_zoning_decline(&fine), None);
        assert!(fine.gpu_supported());
        assert!(!with_mesh.gpu_supported());
    }

    #[test]
    fn prism_sector_and_cylinder_zones_encode() {
        let zones = vec![
            Zone {
                shape: ZoneShape::CoaxialPrism {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Z,
                    n_sides: 3,
                    r_in: 0.0,
                    r_out: 1.0,
                    phase: 0.1,
                },
                absorption: ZoneAbsorption::per_mm(tensor(0.3)),
            },
            Zone {
                shape: ZoneShape::Sector {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Z,
                    angle_from: 0.0,
                    angle_to: 4.0,
                },
                absorption: ZoneAbsorption::per_mm(tensor(0.2)),
            },
        ];
        let table = GpuZoneTable::encode(&zoning_with(zones)).expect("encodable");
        assert_eq!(table.shapes[0].kind, K_PRISM);
        assert_eq!(table.shapes[0].n_sides, 3);
        assert_eq!(table.shapes[1].kind, K_SECTOR);
        assert_eq!(table.shapes[1].flags & ZONE_FLAG_WIDE, ZONE_FLAG_WIDE);
        assert_eq!(table.shapes[1].flags & ZONE_FLAG_FULL, 0);
    }
}
